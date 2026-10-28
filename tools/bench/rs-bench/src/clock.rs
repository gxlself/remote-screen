//! 两机墙钟对时。
//!
//! `probe` 使用 NTP 式四时间戳，从最低 RTT 的一小部分样本估计
//! **Host 钟 − 控制端钟**。误差上界约为最低 RTT 的一半，再加上链路
//! 不对称：SSH 下最低 RTT 通常约 1–2 ms，局域网 UDP 约 0.3 ms。runner
//! 会在会话前、后各测一次，再线性插值以消除时钟漂移。

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;

/// 本机墙钟的 Unix 微秒数。
pub fn now_us() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_micros().min(u64::MAX as u128) as u64,
        Err(_) => 0,
    }
}

/// 一次 NTP 式四时间戳样本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sample {
    pub t1: u64,
    pub t2: u64,
    pub t3: u64,
    pub t4: u64,
}

impl Sample {
    /// 往返时延 `(t4 − t1) − (t3 − t2)`，单位为微秒。
    pub fn rtt_us(&self) -> i64 {
        i128_to_i64((self.t4 as i128 - self.t1 as i128) - (self.t3 as i128 - self.t2 as i128))
    }

    /// 时钟偏移 `((t2 − t1) + (t3 − t4)) / 2`，单位为微秒。
    ///
    /// Rust 的有符号整数除法向零取整，正好符合协议定义。
    pub fn offset_us(&self) -> i64 {
        i128_to_i64(((self.t2 as i128 - self.t1 as i128) + (self.t3 as i128 - self.t4 as i128)) / 2)
    }
}

/// 多个样本的偏移估计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Estimate {
    pub t_us: u64,
    pub offset_us: i64,
    pub rtt_us: i64,
    pub spread_us: i64,
    pub used: usize,
}

/// 从样本中选取最低 RTT 的样本并估计时钟偏移。
pub fn estimate(samples: &[Sample], skip: usize) -> Option<Estimate> {
    let mut valid = samples
        .iter()
        .copied()
        .enumerate()
        .skip(skip)
        .filter_map(|(index, sample)| {
            let rtt = sample.rtt_us();
            (rtt >= 0).then_some((index, sample, rtt))
        })
        .collect::<Vec<_>>();
    if valid.len() < 3 {
        return None;
    }

    valid.sort_by_key(|(index, _sample, rtt)| (*rtt, *index));
    let rounded_tenth = valid.len() / 10 + usize::from(!valid.len().is_multiple_of(10));
    let k = rounded_tenth.max(3).min(valid.len());

    let mut offsets = valid[..k]
        .iter()
        .map(|(_, sample, _)| sample.offset_us())
        .collect::<Vec<_>>();
    offsets.sort_unstable();

    let min_offset = offsets[0];
    let max_offset = offsets[k - 1];
    let median = offsets[(k - 1) / 2];
    let (_, min_rtt_sample, min_rtt) = valid[0];

    Some(Estimate {
        t_us: average_us(min_rtt_sample.t1, min_rtt_sample.t4),
        offset_us: median,
        rtt_us: min_rtt,
        spread_us: i128_to_i64(max_offset as i128 - min_offset as i128),
        used: k,
    })
}

/// 在标准输入输出上运行应答循环。
///
/// `now` 可注入测试时钟。返回成功应答的 `R` 行数。
pub fn serve_lines<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    now: &mut dyn FnMut() -> u64,
) -> io::Result<u64> {
    let mut response_count = 0_u64;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(response_count);
        }

        let clean = trim_line_end(&line);
        if clean == "Q" {
            return Ok(response_count);
        }

        let Some((seq, t1)) = parse_stdio_request(clean) else {
            output.write_all(b"E bad\n")?;
            output.flush()?;
            continue;
        };

        let t2 = now();
        let t3 = now();
        let response = format!("R {seq} {t1} {t2} {t3}\n");
        output.write_all(response.as_bytes())?;
        output.flush()?;
        response_count = response_count.saturating_add(1);
    }
}

/// 在给定 UDP socket 上运行应答循环。
///
/// socket 的读取超时固定为 200 ms，因此当 `stop` 置为 true 后最多在这一
/// 个轮询周期内返回。返回成功应答的数据报数量。
pub fn serve_udp_socket(sock: UdpSocket, stop: Arc<AtomicBool>) -> io::Result<u64> {
    serve_udp_socket_with_idle(sock, stop, None)
}

fn serve_udp_socket_with_idle(
    sock: UdpSocket,
    stop: Arc<AtomicBool>,
    idle_exit: Option<Duration>,
) -> io::Result<u64> {
    sock.set_read_timeout(Some(Duration::from_millis(200)))?;
    let mut buffer = [0_u8; 2048];
    let mut response_count = 0_u64;
    let mut last_request = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(response_count);
        }
        if let Some(idle) = idle_exit {
            if last_request.elapsed() >= idle {
                stop.store(true, Ordering::Relaxed);
                return Ok(response_count);
            }
        }

        match sock.recv_from(&mut buffer) {
            Ok((length, peer)) => {
                let Some((seq, t1)) = parse_udp_request(&buffer[..length]) else {
                    continue;
                };
                last_request = Instant::now();
                let t2 = now_us();
                let t3 = now_us();
                let response = format!("RSCK1 R {seq} {t1} {t2} {t3}\n");
                sock.send_to(response.as_bytes(), peer)?;
                response_count = response_count.saturating_add(1);
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

/// 通过 shell 子进程使用 stdio 探测 Host。
pub fn probe_cmd(
    cmd: &str,
    count: usize,
    interval: Duration,
    timeout: Duration,
) -> anyhow::Result<Vec<Sample>> {
    let mut command = shell_command(cmd);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("start clock command: {cmd}"))?;

    let mut child_stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("clock command stdin is unavailable"))?;
    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("clock command stdout is unavailable"))?;

    let (sender, receiver) = mpsc::channel::<io::Result<Option<String>>>();
    let reader_thread = thread::spawn(move || read_child_lines(child_stdout, sender));

    let mut samples = Vec::with_capacity(count);
    let mut failure = None;
    for seq in 0..count {
        if seq != 0 && !interval.is_zero() {
            thread::sleep(interval);
        }
        let t1 = now_us();
        let request = format!("P {seq} {t1}\n");
        if let Err(error) = child_stdin
            .write_all(request.as_bytes())
            .and_then(|_| child_stdin.flush())
        {
            failure = Some(anyhow::anyhow!(error).context("write clock request"));
            break;
        }

        match receive_matching_response(&receiver, timeout, seq as u64, t1) {
            Ok((t2, t3)) => samples.push(Sample {
                t1,
                t2,
                t3,
                t4: now_us(),
            }),
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }

    // A cooperative serve process exits after Q. Ignore a broken pipe here:
    // the process may already have exited because the probe failed.
    let _ = child_stdin.write_all(b"Q\n");
    let _ = child_stdin.flush();
    drop(child_stdin);

    let wait_error = wait_for_child(&mut child);
    let _ = reader_thread.join();

    if let Some(error) = failure {
        return Err(error);
    }
    if let Some(error) = wait_error {
        return Err(error);
    }
    Ok(samples)
}

/// 通过 UDP 探测 Host。
pub fn probe_udp(
    addr: SocketAddr,
    count: usize,
    interval: Duration,
    timeout: Duration,
) -> anyhow::Result<Vec<Sample>> {
    let bind_addr = if addr.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let socket = UdpSocket::bind(bind_addr).context("bind clock probe socket")?;
    socket
        .set_read_timeout(Some(timeout))
        .context("set clock probe timeout")?;

    let mut buffer = [0_u8; 2048];
    let mut samples = Vec::with_capacity(count);
    for seq in 0..count {
        if seq != 0 && !interval.is_zero() {
            thread::sleep(interval);
        }
        socket
            .set_read_timeout(Some(timeout))
            .context("reset clock probe timeout")?;
        let t1 = now_us();
        let request = format!("RSCK1 P {seq} {t1}\n");
        socket
            .send_to(request.as_bytes(), addr)
            .with_context(|| format!("send clock request to {addr}"))?;

        let started = Instant::now();
        loop {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                bail!(
                    "clock UDP response timed out after {} ms",
                    timeout.as_millis()
                );
            }
            socket
                .set_read_timeout(Some(remaining))
                .context("update clock probe timeout")?;
            match socket.recv_from(&mut buffer) {
                Ok((length, peer)) => {
                    if peer != addr {
                        continue;
                    }
                    let Some((response_seq, response_t1, t2, t3)) =
                        parse_udp_response(&buffer[..length])
                    else {
                        continue;
                    };
                    if response_seq != seq as u64 || response_t1 != t1 {
                        continue;
                    }
                    samples.push(Sample {
                        t1,
                        t2,
                        t3,
                        t4: now_us(),
                    });
                    break;
                }
                Err(error)
                    if error.kind() == io::ErrorKind::TimedOut
                        || error.kind() == io::ErrorKind::WouldBlock =>
                {
                    bail!(
                        "clock UDP response timed out after {} ms",
                        timeout.as_millis()
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error).context("read clock UDP response"),
            }
        }
    }
    Ok(samples)
}

#[derive(Debug, Parser)]
#[command(name = "rs-bench clock", about = "measure Host/control clock offset")]
struct ClockArgs {
    #[command(subcommand)]
    command: ClockCommand,
}

#[derive(Debug, Subcommand)]
enum ClockCommand {
    Serve(ServeArgs),
    Probe(ProbeArgs),
}

#[derive(Debug, Args)]
struct ServeArgs {
    #[command(flatten)]
    transport: ServeTransport,
    #[arg(long, value_name = "SECONDS", requires = "udp")]
    idle_exit: Option<u64>,
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
struct ServeTransport {
    #[arg(long)]
    stdio: bool,
    #[arg(long, value_name = "ADDR")]
    udp: Option<SocketAddr>,
}

#[derive(Debug, Args)]
struct ProbeArgs {
    #[command(flatten)]
    transport: ProbeTransport,
    #[arg(long, default_value_t = 200)]
    count: usize,
    #[arg(long, default_value_t = 5)]
    interval_ms: u64,
    #[arg(long, default_value_t = 5)]
    skip: usize,
    #[arg(long, default_value_t = 2_000)]
    timeout_ms: u64,
    #[arg(long, value_name = "PATH|-")]
    json: Option<String>,
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
struct ProbeTransport {
    #[arg(long, value_name = "CMD")]
    cmd: Option<String>,
    #[arg(long, value_name = "ADDR")]
    udp: Option<SocketAddr>,
}

#[derive(Debug, Serialize)]
struct ClockJson {
    schema: u8,
    method: &'static str,
    count: usize,
    ok: usize,
    t_us: u64,
    offset_us: i64,
    rtt_us: i64,
    spread_us: i64,
    used: usize,
}

/// `rs-bench clock` 的命令行入口。
pub fn cli(args: Vec<String>) -> anyhow::Result<i32> {
    let parsed = match ClockArgs::try_parse_from(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return Ok(code);
        }
    };

    match parsed.command {
        ClockCommand::Serve(options) => serve_cli(options),
        ClockCommand::Probe(options) => probe_cli(options),
    }
}

fn serve_cli(options: ServeArgs) -> anyhow::Result<i32> {
    if options.idle_exit.is_some() && options.transport.udp.is_none() {
        eprintln!("error: --idle-exit requires --udp");
        return Ok(2);
    }
    if options.transport.stdio {
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut now = now_us;
        serve_lines(stdin.lock(), stdout.lock(), &mut now).context("serve clock over stdio")?;
        return Ok(0);
    }

    let addr = options
        .transport
        .udp
        .ok_or_else(|| anyhow::anyhow!("one of --stdio or --udp is required"))?;
    let socket =
        UdpSocket::bind(addr).with_context(|| format!("bind clock UDP socket at {addr}"))?;
    let actual_addr = socket
        .local_addr()
        .context("get clock UDP socket address")?;
    eprintln!("listening {actual_addr}");
    let stop = Arc::new(AtomicBool::new(false));
    let idle = options.idle_exit.map(Duration::from_secs);
    serve_udp_socket_with_idle(socket, stop, idle).context("serve clock over UDP")?;
    Ok(0)
}

fn probe_cli(options: ProbeArgs) -> anyhow::Result<i32> {
    let method = if options.transport.cmd.is_some() {
        "stdio"
    } else {
        "udp"
    };
    let interval = Duration::from_millis(options.interval_ms);
    let timeout = Duration::from_millis(options.timeout_ms);
    let samples = match (options.transport.cmd.as_deref(), options.transport.udp) {
        (Some(command), None) => match probe_cmd(command, options.count, interval, timeout) {
            Ok(samples) => samples,
            Err(error) => {
                eprintln!("clock probe failed: {error:#}");
                return Ok(1);
            }
        },
        (None, Some(addr)) => match probe_udp(addr, options.count, interval, timeout) {
            Ok(samples) => samples,
            Err(error) => {
                eprintln!("clock probe failed: {error:#}");
                return Ok(1);
            }
        },
        _ => {
            eprintln!("clock probe requires exactly one of --cmd or --udp");
            return Ok(1);
        }
    };

    let Some(result) = estimate(&samples, options.skip) else {
        eprintln!(
            "clock probe failed: insufficient valid samples (ok {} of {}, skip {})",
            samples.len(),
            options.count,
            options.skip
        );
        return Ok(1);
    };
    let minimum_ok = options.count / 2;
    if samples.len() < minimum_ok {
        eprintln!(
            "clock probe failed: only {} of {} samples succeeded (need {})",
            samples.len(),
            options.count,
            minimum_ok
        );
        return Ok(1);
    }

    if let Some(path) = options.json.as_deref() {
        let document = ClockJson {
            schema: 1,
            method,
            count: options.count,
            ok: samples.len(),
            t_us: result.t_us,
            offset_us: result.offset_us,
            rtt_us: result.rtt_us,
            spread_us: result.spread_us,
            used: result.used,
        };
        let encoded = serde_json::to_string(&document).context("encode clock JSON")?;
        if path == "-" {
            let mut stdout = io::stdout().lock();
            stdout.write_all(encoded.as_bytes())?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        } else {
            let mut file = std::fs::File::create(path)
                .with_context(|| format!("create clock JSON output {path}"))?;
            file.write_all(encoded.as_bytes())?;
            file.write_all(b"\n")?;
            file.flush()?;
        }
    } else {
        println!(
            "offset {} us, rtt min {} us, spread {} us (used {} of {})",
            result.offset_us,
            result.rtt_us,
            result.spread_us,
            result.used,
            samples.len()
        );
    }
    Ok(0)
}

fn parse_stdio_request(line: &str) -> Option<(u64, u64)> {
    if !line.is_ascii() {
        return None;
    }
    let mut fields = line.split_whitespace();
    if fields.next()? != "P" {
        return None;
    }
    let seq = fields.next()?.parse().ok()?;
    let t1 = fields.next()?.parse().ok()?;
    fields.next().is_none().then_some((seq, t1))
}

fn parse_stdio_response(line: &str) -> Option<(u64, u64, u64, u64)> {
    if !line.is_ascii() {
        return None;
    }
    let mut fields = line.split_whitespace();
    if fields.next()? != "R" {
        return None;
    }
    let seq = fields.next()?.parse().ok()?;
    let t1 = fields.next()?.parse().ok()?;
    let t2 = fields.next()?.parse().ok()?;
    let t3 = fields.next()?.parse().ok()?;
    fields.next().is_none().then_some((seq, t1, t2, t3))
}

fn parse_udp_request(data: &[u8]) -> Option<(u64, u64)> {
    let line = std::str::from_utf8(data)
        .ok()?
        .trim_end_matches(&['\r', '\n'][..]);
    if !line.is_ascii() {
        return None;
    }
    let mut fields = line.split_whitespace();
    if fields.next()? != "RSCK1" || fields.next()? != "P" {
        return None;
    }
    let seq = fields.next()?.parse().ok()?;
    let t1 = fields.next()?.parse().ok()?;
    fields.next().is_none().then_some((seq, t1))
}

fn parse_udp_response(data: &[u8]) -> Option<(u64, u64, u64, u64)> {
    let line = std::str::from_utf8(data)
        .ok()?
        .trim_end_matches(&['\r', '\n'][..]);
    if !line.is_ascii() {
        return None;
    }
    let mut fields = line.split_whitespace();
    if fields.next()? != "RSCK1" || fields.next()? != "R" {
        return None;
    }
    let seq = fields.next()?.parse().ok()?;
    let t1 = fields.next()?.parse().ok()?;
    let t2 = fields.next()?.parse().ok()?;
    let t3 = fields.next()?.parse().ok()?;
    fields.next().is_none().then_some((seq, t1, t2, t3))
}

fn trim_line_end(line: &str) -> &str {
    line.trim_end_matches(&['\r', '\n'][..])
}

fn average_us(left: u64, right: u64) -> u64 {
    ((left as u128 + right as u128) / 2) as u64
}

fn i128_to_i64(value: i128) -> i64 {
    if value > i64::MAX as i128 {
        i64::MAX
    } else if value < i64::MIN as i128 {
        i64::MIN
    } else {
        value as i64
    }
}

fn shell_command(cmd: &str) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(cmd);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("sh");
        command.arg("-c").arg(cmd);
        command
    }
}

fn read_child_lines<R: Read>(stdout: R, sender: mpsc::Sender<io::Result<Option<String>>>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => {
                let _ = sender.send(Ok(None));
                return;
            }
            Ok(_) => {
                if sender.send(Ok(Some(line))).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        }
    }
}

fn receive_matching_response(
    receiver: &mpsc::Receiver<io::Result<Option<String>>>,
    timeout: Duration,
    expected_seq: u64,
    expected_t1: u64,
) -> anyhow::Result<(u64, u64)> {
    let started = Instant::now();
    loop {
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            bail!(
                "clock stdio response timed out after {} ms",
                timeout.as_millis()
            );
        }
        match receiver.recv_timeout(remaining) {
            Ok(Ok(Some(line))) => {
                let clean = trim_line_end(&line);
                let Some((seq, t1, t2, t3)) = parse_stdio_response(clean) else {
                    continue;
                };
                if seq == expected_seq && t1 == expected_t1 {
                    return Ok((t2, t3));
                }
            }
            Ok(Ok(None)) => bail!("clock stdio command exited before replying"),
            Ok(Err(error)) => return Err(error).context("read clock stdio response"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                bail!(
                    "clock stdio response timed out after {} ms",
                    timeout.as_millis()
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("clock stdio output reader disconnected");
            }
        }
    }
}

fn wait_for_child(child: &mut Child) -> Option<anyhow::Error> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return None;
                }
                return Some(anyhow::anyhow!("clock command exited with status {status}"));
            }
            Ok(None) => {
                let now = Instant::now();
                if now >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Some(anyhow::anyhow!(
                        "clock command did not exit within 3 seconds"
                    ));
                }
                let remaining = deadline.saturating_duration_since(now);
                thread::sleep(remaining.min(Duration::from_millis(10)));
            }
            Err(error) => return Some(anyhow::anyhow!(error).context("wait for clock command")),
        }
    }
}
