//! tailscaled LocalAPI client with a CLI fallback.
//!
//! The LocalAPI implementation intentionally uses a small HTTP/1.1 codec over
//! a Unix socket or Windows named pipe. It needs only `Content-Length` and
//! `chunked` responses, so a
//! full HTTP client would add dependencies and surface area without helping
//! this local, two-request protocol.
//!
//! Windows uses `\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled`,
//! accessible only to administrators and SYSTEM. The service runs as SYSTEM;
//! whether tailscaled accepts SYSTEM's WhoIs requests needs verification on a
//! real Windows host (manual probe 1). Rejected requests fall back to the CLI.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{Mutex, OnceCell};
use tokio::time::timeout;
use tracing::warn;

const LOCAL_TIMEOUT: Duration = Duration::from_secs(2);
const CLI_TIMEOUT: Duration = Duration::from_secs(3);
const CACHE_TTL_US: u64 = 30_000_000;
const DEFAULT_SOCKET: &str = "/var/run/tailscaled.socket";

/// tailscaled's LocalAPI pipe on Windows.
pub const WINDOWS_LOCALAPI_PIPE: &str =
    r"\\.\pipe\ProtectedPrefix\Administrators\Tailscale\tailscaled";

/// The platform's default tailscaled LocalAPI endpoint.
pub fn default_localapi_path() -> &'static str {
    if cfg!(windows) {
        WINDOWS_LOCALAPI_PIPE
    } else {
        DEFAULT_SOCKET
    }
}

/// Prefer the native 64-bit installation directory, even in a 32-bit process.
/// Build Windows paths as strings so callers can also test them on Unix.
pub fn windows_cli_candidates(
    program_files: Option<&str>,
    program_w6432: Option<&str>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut seen = Vec::new();
    for root in [program_w6432, program_files].into_iter().flatten() {
        if root.is_empty() {
            continue;
        }
        let root = root.trim_end_matches(['\\', '/']);
        let candidate = format!(r"{root}\Tailscale\tailscale.exe");
        let key = candidate.to_lowercase();
        if !seen.contains(&key) {
            seen.push(key);
            candidates.push(PathBuf::from(candidate));
        }
    }
    if candidates.is_empty() {
        candidates.push(PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe"));
    }
    candidates
}

/// Choose the first installed CLI, falling back to PATH lookup.
pub fn pick_cli(candidates: &[PathBuf], exists: impl Fn(&Path) -> bool) -> PathBuf {
    candidates
        .iter()
        .find(|candidate| exists(candidate))
        .cloned()
        .unwrap_or_else(|| PathBuf::from("tailscale"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WhoIs {
    pub login: String,
    pub stable_id: String,
    pub node_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerPath {
    pub direct: bool,
    pub relay: Option<String>,
    pub cur_addr: Option<SocketAddr>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WhoIsError {
    #[error("tailscale local API I/O error: {0}")]
    Io(String),
    #[error("tailscale local API request timed out")]
    Timeout,
    #[error("tailscale local API returned HTTP {0}")]
    HttpStatus(u16),
    #[error("invalid tailscale JSON: {0}")]
    Json(String),
    #[error("tailscale CLI failed: {0}")]
    Command(String),
    #[error("tailscale status has no peer for {0}")]
    PeerNotFound(IpAddr),
}

#[async_trait]
pub trait WhoIsSource: Send + Sync {
    async fn whois(&self, peer: SocketAddr) -> Result<WhoIs, WhoIsError>;
    async fn peer_path(&self, peer_ip: IpAddr) -> Result<PeerPath, WhoIsError>;

    /// Optional local identity lookup, independent of peer caches.
    async fn self_login(&self) -> Result<String, WhoIsError> {
        Err(WhoIsError::Json("Self user lookup unavailable".into()))
    }
}

#[derive(Clone, Debug)]
pub struct LocalApiSource {
    socket_path: PathBuf,
}

impl Default for LocalApiSource {
    fn default() -> Self {
        Self::new(default_localapi_path())
    }
}

impl LocalApiSource {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            socket_path: path.as_ref().to_path_buf(),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn self_login(&self) -> Result<String, WhoIsError> {
        parse_self_login(&self.get("/localapi/v0/status").await?)
    }

    async fn get(&self, path: &str) -> Result<Value, WhoIsError> {
        let socket_path = self.socket_path.clone();
        let path = path.to_owned();
        let response = timeout(LOCAL_TIMEOUT, async move {
            request_local_api(socket_path, &path).await
        })
        .await
        .map_err(|_| WhoIsError::Timeout)??;
        serde_json::from_slice(&response).map_err(|error| WhoIsError::Json(error.to_string()))
    }
}

#[async_trait]
impl WhoIsSource for LocalApiSource {
    async fn self_login(&self) -> Result<String, WhoIsError> {
        LocalApiSource::self_login(self).await
    }

    async fn whois(&self, peer: SocketAddr) -> Result<WhoIs, WhoIsError> {
        // Tailscale accepts an unescaped addr query. Brackets keep IPv6 host
        // and port unambiguous while preserving the documented endpoint.
        let addr = format_addr(peer);
        let value = self.get(&format!("/localapi/v0/whois?addr={addr}")).await?;
        parse_whois(&value)
    }

    async fn peer_path(&self, peer_ip: IpAddr) -> Result<PeerPath, WhoIsError> {
        let value = self.get("/localapi/v0/status").await?;
        parse_peer_path(&value, peer_ip)
    }
}

#[derive(Clone, Debug)]
pub struct CliSource {
    executable: PathBuf,
}

impl Default for CliSource {
    fn default() -> Self {
        #[cfg(windows)]
        {
            let program_files = std::env::var("ProgramFiles").ok();
            let program_w6432 = std::env::var("ProgramW6432").ok();
            Self::new(pick_cli(
                &windows_cli_candidates(program_files.as_deref(), program_w6432.as_deref()),
                |path| path.is_file(),
            ))
        }
        #[cfg(not(windows))]
        Self::new("tailscale")
    }
}

impl CliSource {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            executable: path.as_ref().to_path_buf(),
        }
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub async fn self_login(&self) -> Result<String, WhoIsError> {
        let status = self
            .run_json(&["status".to_owned(), "--json".to_owned()])
            .await?;
        parse_self_login(&status)
    }

    async fn run_json(&self, args: &[String]) -> Result<Value, WhoIsError> {
        let mut command = Command::new(&self.executable);
        for arg in args {
            // Each argument is passed separately; no shell is involved.
            command.arg(arg);
        }
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::null());
        command.kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let child = command
            .spawn()
            .map_err(|error| WhoIsError::Command(error.to_string()))?;
        // Drain stdout while waiting, including when it exceeds the pipe's
        // buffer. Dropping this future on timeout also kills the owned child.
        let output = timeout(CLI_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| WhoIsError::Timeout)?
            .map_err(|error| WhoIsError::Command(error.to_string()))?;
        if !output.status.success() {
            return Err(WhoIsError::Command(format!(
                "exit status {}",
                output.status
            )));
        }
        serde_json::from_slice(&output.stdout).map_err(|error| WhoIsError::Json(error.to_string()))
    }
}

#[async_trait]
impl WhoIsSource for CliSource {
    async fn self_login(&self) -> Result<String, WhoIsError> {
        CliSource::self_login(self).await
    }

    async fn whois(&self, peer: SocketAddr) -> Result<WhoIs, WhoIsError> {
        let value = self
            .run_json(&[
                "whois".to_owned(),
                "--json".to_owned(),
                peer.ip().to_string(),
            ])
            .await?;
        parse_whois(&value)
    }

    async fn peer_path(&self, peer_ip: IpAddr) -> Result<PeerPath, WhoIsError> {
        let value = self
            .run_json(&["status".to_owned(), "--json".to_owned()])
            .await?;
        parse_peer_path(&value, peer_ip)
    }
}

/// A monotonic-ish wall clock abstraction. Tests can inject a deterministic
/// implementation and advance it without sleeping.
pub trait WhoIsClock: Send + Sync {
    fn now_micros(&self) -> u64;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl WhoIsClock for SystemClock {
    fn now_micros(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| {
                duration.as_micros().min(u64::MAX as u128) as u64
            })
    }
}

#[derive(Clone)]
struct Cached<T> {
    at_us: u64,
    value: T,
}

type WhoisFlight = Arc<OnceCell<Result<WhoIs, WhoIsError>>>;
type PathFlight = Arc<OnceCell<Result<PeerPath, WhoIsError>>>;

/// LocalAPI-first WhoIs source with a 30-second, per-IP single-flight cache.
#[derive(Clone)]
pub struct ChainedWhoIs {
    local: Arc<dyn WhoIsSource>,
    cli: Arc<dyn WhoIsSource>,
    clock: Arc<dyn WhoIsClock>,
    whois_cache: Arc<Mutex<HashMap<IpAddr, Cached<WhoIs>>>>,
    path_cache: Arc<Mutex<HashMap<IpAddr, Cached<PeerPath>>>>,
    whois_flights: Arc<Mutex<HashMap<IpAddr, WhoisFlight>>>,
    path_flights: Arc<Mutex<HashMap<IpAddr, PathFlight>>>,
}

impl fmt::Debug for ChainedWhoIs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChainedWhoIs")
            .finish_non_exhaustive()
    }
}

impl ChainedWhoIs {
    /// Query fresh status on every attempt, including after logout/login.
    pub async fn self_login(&self) -> Result<String, WhoIsError> {
        match self.local.self_login().await {
            Ok(login) => Ok(login),
            Err(_) => self.cli.self_login().await,
        }
    }

    pub fn new(local: LocalApiSource, cli: CliSource) -> Self {
        Self::with_sources(Arc::new(local), Arc::new(cli))
    }

    pub fn with_sources(local: Arc<dyn WhoIsSource>, cli: Arc<dyn WhoIsSource>) -> Self {
        Self::with_clock(local, cli, Arc::new(SystemClock))
    }

    pub fn with_clock(
        local: Arc<dyn WhoIsSource>,
        cli: Arc<dyn WhoIsSource>,
        clock: Arc<dyn WhoIsClock>,
    ) -> Self {
        Self {
            local,
            cli,
            clock,
            whois_cache: Arc::new(Mutex::new(HashMap::new())),
            path_cache: Arc::new(Mutex::new(HashMap::new())),
            whois_flights: Arc::new(Mutex::new(HashMap::new())),
            path_flights: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn whois_outcome(&self, peer: SocketAddr) -> crate::auth::WhoIsOutcome {
        match self.whois(peer).await {
            Ok(value) => crate::auth::WhoIsOutcome::Ok {
                login: value.login,
                stable_id: value.stable_id,
                node_name: value.node_name,
            },
            Err(error) => {
                warn!(error = %error, "tailscale WhoIs unavailable");
                crate::auth::WhoIsOutcome::Unavailable
            }
        }
    }

    async fn query_whois(&self, peer: SocketAddr) -> Result<WhoIs, WhoIsError> {
        match self.local.whois(peer).await {
            Ok(value) => Ok(value),
            Err(local_error) => {
                warn!(error = %local_error, "tailscale LocalAPI WhoIs failed; trying CLI");
                self.cli.whois(peer).await.map_err(|cli_error| {
                    warn!(error = %cli_error, "tailscale CLI WhoIs failed");
                    cli_error
                })
            }
        }
    }

    async fn query_path(&self, peer_ip: IpAddr) -> Result<PeerPath, WhoIsError> {
        match self.local.peer_path(peer_ip).await {
            Ok(value) => Ok(value),
            Err(local_error) => {
                warn!(error = %local_error, "tailscale LocalAPI status failed; trying CLI");
                self.cli.peer_path(peer_ip).await.map_err(|cli_error| {
                    warn!(error = %cli_error, "tailscale CLI status failed");
                    cli_error
                })
            }
        }
    }

    fn fresh<T>(entry: &Cached<T>, now: u64) -> bool {
        now.saturating_sub(entry.at_us) < CACHE_TTL_US
    }

    pub async fn clear_cache(&self) {
        self.whois_cache.lock().await.clear();
        self.path_cache.lock().await.clear();
    }
}

#[async_trait]
impl WhoIsSource for ChainedWhoIs {
    async fn self_login(&self) -> Result<String, WhoIsError> {
        ChainedWhoIs::self_login(self).await
    }

    async fn whois(&self, peer: SocketAddr) -> Result<WhoIs, WhoIsError> {
        let ip = peer.ip();
        let now = self.clock.now_micros();
        if let Some(value) = self
            .whois_cache
            .lock()
            .await
            .get(&ip)
            .filter(|entry| Self::fresh(entry, now))
            .map(|entry| entry.value.clone())
        {
            return Ok(value);
        }

        let cell = {
            let mut flights = self.whois_flights.lock().await;
            flights
                .entry(ip)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let result = cell
            .get_or_init(|| async { self.query_whois(peer).await })
            .await
            .clone();
        {
            let mut flights = self.whois_flights.lock().await;
            if flights
                .get(&ip)
                .is_some_and(|current| Arc::ptr_eq(current, &cell))
            {
                flights.remove(&ip);
            }
        }
        if let Ok(value) = &result {
            self.whois_cache.lock().await.insert(
                ip,
                Cached {
                    at_us: self.clock.now_micros(),
                    value: value.clone(),
                },
            );
        }
        result
    }

    async fn peer_path(&self, peer_ip: IpAddr) -> Result<PeerPath, WhoIsError> {
        let now = self.clock.now_micros();
        if let Some(value) = self
            .path_cache
            .lock()
            .await
            .get(&peer_ip)
            .filter(|entry| Self::fresh(entry, now))
            .map(|entry| entry.value.clone())
        {
            return Ok(value);
        }

        let cell = {
            let mut flights = self.path_flights.lock().await;
            flights
                .entry(peer_ip)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let result = cell
            .get_or_init(|| async { self.query_path(peer_ip).await })
            .await
            .clone();
        {
            let mut flights = self.path_flights.lock().await;
            if flights
                .get(&peer_ip)
                .is_some_and(|current| Arc::ptr_eq(current, &cell))
            {
                flights.remove(&peer_ip);
            }
        }
        if let Ok(value) = &result {
            self.path_cache.lock().await.insert(
                peer_ip,
                Cached {
                    at_us: self.clock.now_micros(),
                    value: value.clone(),
                },
            );
        }
        result
    }
}

fn io_error(error: std::io::Error) -> WhoIsError {
    WhoIsError::Io(error.to_string())
}

fn format_addr(peer: SocketAddr) -> String {
    match peer {
        SocketAddr::V4(_) => peer.to_string(),
        SocketAddr::V6(address) => format!("[{}]:{}", address.ip(), address.port()),
    }
}

#[cfg(unix)]
async fn request_local_api(socket_path: PathBuf, path: &str) -> Result<Vec<u8>, WhoIsError> {
    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .map_err(io_error)?;
    request(stream, path).await
}

#[cfg(windows)]
async fn request_local_api(socket_path: PathBuf, path: &str) -> Result<Vec<u8>, WhoIsError> {
    let stream = loop {
        match tokio::net::windows::named_pipe::ClientOptions::new().open(&socket_path) {
            Ok(stream) => break stream,
            Err(error) if error.raw_os_error() == Some(231) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(io_error(error)),
        }
    };
    request(stream, path).await
}

#[cfg(not(any(unix, windows)))]
async fn request_local_api(_socket_path: PathBuf, _path: &str) -> Result<Vec<u8>, WhoIsError> {
    Err(WhoIsError::Io(
        "LocalAPI is not supported on this platform".into(),
    ))
}

async fn request<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    path: &str,
) -> Result<Vec<u8>, WhoIsError> {
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: local-tailscaled.sock\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(io_error)?;
    read_http_response(&mut stream).await
}

async fn read_http_response<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Vec<u8>, WhoIsError> {
    let mut header = Vec::with_capacity(1024);
    let mut one = [0_u8; 1];
    loop {
        stream.read_exact(&mut one).await.map_err(io_error)?;
        header.push(one[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
        if header.len() > 64 * 1024 {
            return Err(WhoIsError::Json("HTTP headers too large".to_owned()));
        }
    }
    let header_text = std::str::from_utf8(&header)
        .map_err(|error| WhoIsError::Json(format!("invalid HTTP headers: {error}")))?;
    let mut lines = header_text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| WhoIsError::Json("missing HTTP status".to_owned()))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| WhoIsError::Json("missing HTTP status code".to_owned()))?
        .parse::<u16>()
        .map_err(|error| WhoIsError::Json(format!("invalid HTTP status: {error}")))?;
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = Some(value.trim().parse::<usize>().map_err(|error| {
                    WhoIsError::Json(format!("invalid Content-Length: {error}"))
                })?);
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value.trim().to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }
    if !(200..300).contains(&status) {
        return Err(WhoIsError::HttpStatus(status));
    }
    if let Some(length) = content_length {
        let mut body = vec![0_u8; length];
        stream.read_exact(&mut body).await.map_err(io_error)?;
        return Ok(body);
    }
    if chunked {
        let mut body = Vec::new();
        loop {
            let line = read_line(stream).await?;
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16)
                .map_err(|error| WhoIsError::Json(format!("invalid chunk length: {error}")))?;
            if size == 0 {
                // Consume trailers up to the blank line.
                loop {
                    if read_line(stream).await?.is_empty() {
                        break;
                    }
                }
                break;
            }
            let old_len = body.len();
            body.resize(old_len + size, 0);
            stream
                .read_exact(&mut body[old_len..])
                .await
                .map_err(io_error)?;
            let mut crlf = [0_u8; 2];
            stream.read_exact(&mut crlf).await.map_err(io_error)?;
            if crlf != *b"\r\n" {
                return Err(WhoIsError::Json("invalid chunk terminator".to_owned()));
            }
        }
        return Ok(body);
    }
    let mut body = Vec::new();
    stream.read_to_end(&mut body).await.map_err(io_error)?;
    Ok(body)
}

async fn read_line<S: AsyncRead + Unpin>(stream: &mut S) -> Result<String, WhoIsError> {
    let mut line = Vec::new();
    let mut one = [0_u8; 1];
    loop {
        stream.read_exact(&mut one).await.map_err(io_error)?;
        if one[0] == b'\n' {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return String::from_utf8(line)
                .map_err(|error| WhoIsError::Json(format!("invalid chunk header: {error}")));
        }
        line.push(one[0]);
        if line.len() > 64 * 1024 {
            return Err(WhoIsError::Json("HTTP line too large".to_owned()));
        }
    }
}

/// Test-only entry point for exercising the LocalAPI HTTP response parser.
#[doc(hidden)]
pub async fn read_http_response_for_test<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> Result<Vec<u8>, WhoIsError> {
    read_http_response(stream).await
}

fn parse_whois(value: &Value) -> Result<WhoIs, WhoIsError> {
    let node = value
        .get("Node")
        .ok_or_else(|| WhoIsError::Json("WhoIs response has no Node".to_owned()))?;
    let profile = value
        .get("UserProfile")
        .ok_or_else(|| WhoIsError::Json("WhoIs response has no UserProfile".to_owned()))?;
    Ok(WhoIs {
        login: string_field(profile, &["LoginName", "login", "login_name"])?,
        stable_id: string_field(node, &["StableID", "stable_id", "StableId"])?,
        node_name: string_field(node, &["Name", "name"])?,
    })
}

fn string_field(value: &Value, names: &[&str]) -> Result<String, WhoIsError> {
    for name in names {
        if let Some(field) = value.get(*name) {
            if let Some(string) = field.as_str() {
                return Ok(string.to_owned());
            }
            if field.is_number() {
                return Ok(field.to_string());
            }
        }
    }
    Err(WhoIsError::Json(format!("missing JSON field {}", names[0])))
}

fn parse_peer_path(value: &Value, peer_ip: IpAddr) -> Result<PeerPath, WhoIsError> {
    let peer = find_peer(value.get("Peer").unwrap_or(value), peer_ip)
        .ok_or(WhoIsError::PeerNotFound(peer_ip))?;
    let cur_addr = match peer.get("CurAddr").and_then(Value::as_str) {
        Some(value) if !value.is_empty() => Some(
            value
                .parse::<SocketAddr>()
                .map_err(|error| WhoIsError::Json(format!("invalid CurAddr: {error}")))?,
        ),
        _ => None,
    };
    let relay = peer
        .get("Relay")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    Ok(PeerPath {
        direct: cur_addr.is_some(),
        relay,
        cur_addr,
    })
}

fn find_peer(value: &Value, peer_ip: IpAddr) -> Option<&Value> {
    if let Some(peers) = value.as_array() {
        for peer in peers {
            if has_ip(peer, peer_ip) {
                return Some(peer);
            }
            if let Some(found) = find_peer(peer, peer_ip) {
                return Some(found);
            }
        }
    } else if let Some(object) = value.as_object() {
        if has_ip(value, peer_ip) {
            return Some(value);
        }
        for child in object.values() {
            if let Some(found) = find_peer(child, peer_ip) {
                return Some(found);
            }
        }
    }
    None
}

fn has_ip(value: &Value, peer_ip: IpAddr) -> bool {
    value
        .get("TailscaleIPs")
        .and_then(Value::as_array)
        .is_some_and(|ips| {
            ips.iter()
                .filter_map(Value::as_str)
                .filter_map(|ip| ip.parse::<IpAddr>().ok())
                .any(|ip| ip == peer_ip)
        })
}

// TODO(H3): Tailscale App edition's TCP LocalAPI port/token transport.

/// Resolve only the local Self user; tagged nodes have no usable login.
pub fn parse_self_login(status: &Value) -> Result<String, WhoIsError> {
    let user_id = status
        .get("Self")
        .and_then(|self_node| self_node.get("UserID"))
        .and_then(Value::as_u64)
        .filter(|id| *id != 0)
        .ok_or_else(|| WhoIsError::Json("missing nonzero Self.UserID".into()))?;
    status
        .get("User")
        .and_then(|users| users.get(user_id.to_string()))
        .and_then(|user| user.get("LoginName"))
        .and_then(Value::as_str)
        .filter(|login| !login.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| WhoIsError::Json("missing Self user LoginName".into()))
}
