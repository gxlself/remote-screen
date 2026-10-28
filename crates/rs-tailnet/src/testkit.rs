//! Synchronous local testnet helper. No async runtime or HTTP client is required.

use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct TestPaths {
    pub lib: PathBuf,
    pub testnet: PathBuf,
}

pub fn paths_or_skip(test: &str) -> Option<TestPaths> {
    let lib = std::env::var_os("RS_TAILNET_LIB").filter(|s| !s.is_empty());
    let testnet = std::env::var_os("RS_TAILNET_TESTNET").filter(|s| !s.is_empty());
    match (lib, testnet) {
        (Some(lib), Some(testnet)) => Some(TestPaths {
            lib: lib.into(),
            testnet: testnet.into(),
        }),
        _ => {
            assert!(
                std::env::var("RS_REQUIRE_TAILNET").as_deref() != Ok("1"),
                "{test}: RS_REQUIRE_TAILNET=1 needs RS_TAILNET_LIB / RS_TAILNET_TESTNET"
            );
            eprintln!("SKIP {test}: RS_TAILNET_LIB / RS_TAILNET_TESTNET not set");
            None
        }
    }
}

#[derive(Default)]
pub struct TestNetOptions {
    pub require_auth: bool,
    pub auth_key: Option<String>,
    pub forward_udp: Vec<(u16, SocketAddr)>,
    pub forward_tcp: Vec<(u16, SocketAddr)>,
}

pub struct TestNet {
    pub control_url: String,
    pub admin_url: String,
    pub auth_key: String,
    pub peer_hostname: String,
    pub peer_dns_name: String,
    pub peer_ips: Vec<IpAddr>,
    pub udp_echo: u16,
    pub tcp_echo: u16,
    process: Process,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Announcement {
    control_url: String,
    admin_url: String,
    auth_key: String,
    peer: Peer,
    udp_echo: u16,
    tcp_echo: u16,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Peer {
    hostname: String,
    dns_name: String,
    ips: Vec<IpAddr>,
}

const TAIL_LIMIT: usize = 16 * 1024;
type Tail = Arc<Mutex<VecDeque<u8>>>;

fn tail_push(tail: &Tail, bytes: &[u8]) {
    let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
    tail.extend(bytes.iter().copied());
    let excess = tail.len().saturating_sub(TAIL_LIMIT);
    tail.drain(..excess);
}

fn redact(value: &str, secrets: &[&str]) -> String {
    let mut value = value.to_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        value = value.replace(*secret, "<redacted>");
    }
    // Also cover an auto-generated helper key before its announcement is parsed.
    while let Some(start) = value.find("tskey-") {
        let end = value[start..]
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '}'))
            .map_or(value.len(), |length| start + length);
        value.replace_range(start..end, "<redacted>");
    }
    value
}

struct Process {
    child: Child,
    readers: Vec<JoinHandle<()>>,
    stdout: Tail,
    stderr: Tail,
    secrets: Vec<String>,
}

impl Process {
    fn failure(&mut self, kind: io::ErrorKind, message: &str) -> io::Error {
        self.shutdown();
        let read = |tail: &Tail| {
            let bytes: Vec<_> = tail
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .copied()
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        };
        let secrets: Vec<_> = self.secrets.iter().map(String::as_str).collect();
        io::Error::new(
            kind,
            redact(
                &format!(
                    "{message}; stdout tail: {}; stderr tail: {}",
                    read(&self.stdout),
                    read(&self.stderr)
                ),
                &secrets,
            ),
        )
    }
}

impl Process {
    fn shutdown(&mut self) {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl TestNet {
    pub fn spawn(paths: &TestPaths, opts: &TestNetOptions) -> io::Result<Self> {
        let mut command = Command::new(&paths.testnet);
        command
            .args([
                "-test.run",
                "^TestServe$",
                "-test.timeout",
                "0",
                "-rs.serve",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if opts.require_auth {
            command.arg("-rs.require-auth");
        }
        if let Some(key) = &opts.auth_key {
            command.arg("-rs.authkey").arg(key);
        }
        for (flag, forwards) in [
            ("-rs.forward-udp", &opts.forward_udp),
            ("-rs.forward-tcp", &opts.forward_tcp),
        ] {
            for (port, target) in forwards {
                command.arg(flag).arg(format!("{port}={target}"));
            }
        }
        let mut process = Process {
            child: command.spawn()?,
            readers: Vec::new(),
            stdout: Arc::new(Mutex::new(VecDeque::new())),
            stderr: Arc::new(Mutex::new(VecDeque::new())),
            secrets: opts.auth_key.iter().cloned().collect(),
        };
        if let (Some(root), Ok(task)) = (std::env::var_os("TMPDIR"), std::env::var("RS_TEST_TASK"))
        {
            let file = PathBuf::from(root).join(format!("{}.pids", task.replace(['/', '\\'], "_")));
            let record = format!("{}\n", process.child.id());
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(file)?
                .write_all(record.as_bytes())?;
        }
        let stderr = process
            .child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("missing helper stderr pipe"))?;
        let tail = process.stderr.clone();
        process.readers.push(
            thread::Builder::new()
                .name("rs-testnet-stderr".into())
                .spawn(move || {
                    let mut reader = stderr;
                    let mut buf = [0; 4096];
                    while let Ok(length) = reader.read(&mut buf) {
                        if length == 0 {
                            break;
                        }
                        tail_push(&tail, &buf[..length]);
                    }
                })?,
        );
        let stdout = process
            .child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing helper stdout pipe"))?;
        let tail = process.stdout.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        process.readers.push(
            thread::Builder::new()
                .name("rs-testnet-stdout".into())
                .spawn(move || {
                    let mut reader = BufReader::new(stdout);
                    let mut announced = false;
                    loop {
                        let mut line = String::new();
                        // Bound even malformed helper output; still drain subsequent chunks.
                        match reader.by_ref().take(64 * 1024).read_line(&mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {}
                        }
                        if let Some(json) = line.strip_prefix("RS_TESTNET ") {
                            if !announced {
                                announced = true;
                                let parsed =
                                    serde_json::from_str::<Announcement>(json).map_err(|_| ());
                                let _ = tx.send(parsed);
                            }
                            // Never put the announcement's authKey into diagnostic tails.
                            tail_push(&tail, b"RS_TESTNET <redacted>\n");
                        } else {
                            tail_push(&tail, line.as_bytes());
                        }
                    }
                })?,
        );
        let info = match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(Ok(info)) => info,
            Ok(Err(())) => {
                return Err(process.failure(
                    io::ErrorKind::InvalidData,
                    "invalid RS_TESTNET announcement",
                ))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(process.failure(
                    io::ErrorKind::TimedOut,
                    "no RS_TESTNET announcement within 30 s",
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(process.failure(
                    io::ErrorKind::UnexpectedEof,
                    "helper exited before RS_TESTNET announcement",
                ))
            }
        };
        process.secrets.push(info.auth_key.clone());
        if !info.peer.ips.iter().any(IpAddr::is_ipv4) {
            return Err(process.failure(
                io::ErrorKind::InvalidData,
                "helper peer has no IPv4 address",
            ));
        }
        Ok(Self {
            control_url: info.control_url,
            admin_url: info.admin_url,
            auth_key: info.auth_key,
            peer_hostname: info.peer.hostname,
            peer_dns_name: info.peer.dns_name,
            peer_ips: info.peer.ips,
            udp_echo: info.udp_echo,
            tcp_echo: info.tcp_echo,
            process,
        })
    }

    pub fn peer_ip4(&self) -> Ipv4Addr {
        self.peer_ips
            .iter()
            .find_map(|ip| {
                if let IpAddr::V4(ip) = ip {
                    Some(*ip)
                } else {
                    None
                }
            })
            .expect("TestNet::spawn validated a peer IPv4 address; retain it when editing peer_ips")
    }

    pub fn complete(&self, auth_url: &str) -> io::Result<()> {
        let url = self.admin_url.strip_prefix("http://").ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "admin URL must use http")
        })?;
        let (authority, base) = url.split_once('/').unwrap_or((url, ""));
        let address: SocketAddr = authority.parse().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "admin URL must contain an IP and port",
            )
        })?;
        if !address.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "testnet admin must be loopback",
            ));
        }
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let path = if base.trim_end_matches('/').is_empty() {
            "/complete".to_owned()
        } else {
            format!("/{}/complete", base.trim_end_matches('/'))
        };
        write!(stream, "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", auth_url.len())?;
        stream.write_all(auth_url.as_bytes())?;
        let mut status = String::new();
        BufReader::new(stream).take(4096).read_line(&mut status)?;
        let mut words = status.split_whitespace();
        if !matches!(words.next(), Some("HTTP/1.0" | "HTTP/1.1")) || words.next() != Some("200") {
            return Err(io::Error::other("testnet complete did not return HTTP 200"));
        }
        Ok(())
    }
}

impl Drop for TestNet {
    fn drop(&mut self) {
        // Process owns graceful stdin EOF, bounded wait, kill/wait, and reader joins.
        drop(self.process.child.stdin.take());
    }
}

pub fn temp_dir(tag: &str) -> PathBuf {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp"));
    // Keep names short: tsnet may put Unix sockets beneath this directory.
    let tag = tag.replace(['/', '\\'], "_");
    fs::create_dir_all(&root)
        .unwrap_or_else(|error| panic!("create test root {}: {error}", root.display()));
    loop {
        let path = root.join(format!(
            "rs-{tag}-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create test directory {}: {error}", path.display()),
        }
    }
}
