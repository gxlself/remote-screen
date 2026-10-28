use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use host_core::whois::{
    ChainedWhoIs, CliSource, LocalApiSource, PeerPath, WhoIsClock, WhoIsError, WhoIsSource,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::Notify;

fn temp_path(name: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // nextest runs each test in its own process, so include the pid.
    let path = std::env::temp_dir().join(format!(
        "h3-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

struct FakeClock(AtomicU64);
impl FakeClock {
    fn new() -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(0)))
    }
    fn advance(&self, micros: u64) {
        self.0.fetch_add(micros, Ordering::Relaxed);
    }
}
impl WhoIsClock for FakeClock {
    fn now_micros(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

async fn serve(
    listener: UnixListener,
    responses: Vec<Vec<u8>>,
    count: Arc<AtomicUsize>,
    ready: Arc<Notify>,
) {
    ready.notify_one();
    for body in responses {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request).await.unwrap();
        count.fetch_add(1, Ordering::Relaxed);
        let chunked = body.starts_with(b"WHOIS");
        let body = if chunked { &body[5..] } else { &body[..] };
        if chunked {
            let head = format!(
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
            stream.write_all(b"\r\n0\r\n\r\n").await.unwrap();
        } else {
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            stream.write_all(head.as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
        }
    }
}

fn whois_json() -> Vec<u8> {
    br#"{"Node":{"StableID":"stable-1","Name":"phone-node","Tags":["tag:rs-app"]},"UserProfile":{"LoginName":"alice@","DisplayName":"Alice"}}"#.to_vec()
}

fn tagged_whois() -> Vec<u8> {
    let mut value = b"WHOIS".to_vec();
    value.extend(whois_json());
    value
}

fn status_json() -> Vec<u8> {
    br#"{"Peer":{"key":{"TailscaleIPs":["100.64.0.2"],"CurAddr":"192.0.2.4:41641","Relay":"derp-shanghai"}}}"#.to_vec()
}

#[tokio::test]
async fn local_api_parses_whois_and_direct_derp_status() {
    let dir = temp_path("local");
    let socket = dir.join("tailscaled.socket");
    let listener = UnixListener::bind(&socket).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let ready = Arc::new(Notify::new());
    let task = tokio::spawn(serve(
        listener,
        vec![tagged_whois(), status_json()],
        count.clone(),
        ready.clone(),
    ));
    ready.notified().await;
    let source = LocalApiSource::new(&socket);
    let peer: SocketAddr = "100.64.0.2:7400".parse().unwrap();
    let whois = source.whois(peer).await.unwrap();
    assert_eq!(whois.login, "alice@");
    assert_eq!(whois.stable_id, "stable-1");
    assert_eq!(whois.node_name, "phone-node");
    let path = source.peer_path(peer.ip()).await.unwrap();
    assert_eq!(
        path,
        PeerPath {
            direct: true,
            relay: Some("derp-shanghai".into()),
            cur_addr: Some("192.0.2.4:41641".parse().unwrap())
        }
    );
    assert_eq!(count.load(Ordering::Relaxed), 2);
    task.await.unwrap();
}

#[tokio::test]
async fn local_api_empty_cur_addr_is_derp() {
    let dir = temp_path("derp");
    let socket = dir.join("tailscaled.socket");
    let listener = UnixListener::bind(&socket).unwrap();
    let json =
        br#"{"Peer":{"key":{"TailscaleIPs":["100.64.0.2"],"CurAddr":"","Relay":"derp-tokyo"}}}"#
            .to_vec();
    let count = Arc::new(AtomicUsize::new(0));
    let ready = Arc::new(Notify::new());
    let task = tokio::spawn(serve(listener, vec![json], count, ready.clone()));
    ready.notified().await;
    let path = LocalApiSource::new(&socket)
        .peer_path("100.64.0.2".parse().unwrap())
        .await
        .unwrap();
    assert!(!path.direct);
    assert_eq!(path.cur_addr, None);
    assert_eq!(path.relay.as_deref(), Some("derp-tokyo"));
    task.await.unwrap();
}

fn fake_tailscale(dir: &Path, body: &[u8], sleep: Option<u64>) -> PathBuf {
    let path = dir.join("tailscale");
    let escaped = String::from_utf8_lossy(body);
    let script = if let Some(seconds) = sleep {
        format!("#!/bin/sh\nsleep {seconds}\nprintf '%s\\n' '{escaped}'\n")
    } else {
        format!("#!/bin/sh\nprintf '%s\\n' '{escaped}'\n")
    };
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[tokio::test]
async fn local_api_failure_falls_back_to_cli_and_both_failure_is_error() {
    let dir = temp_path("cli");
    let executable = fake_tailscale(&dir, &whois_json(), None);
    let source = ChainedWhoIs::new(
        LocalApiSource::new(dir.join("missing.sock")),
        CliSource::new(&executable),
    );
    let whois = source
        .whois("100.64.0.2:7400".parse().unwrap())
        .await
        .unwrap();
    assert_eq!(whois.login, "alice@");
    let failed = ChainedWhoIs::new(
        LocalApiSource::new(dir.join("missing.sock")),
        CliSource::new(dir.join("missing-tailscale")),
    );
    assert!(failed
        .whois("100.64.0.2:7400".parse().unwrap())
        .await
        .is_err());
}

#[tokio::test]
async fn cache_expires_and_concurrent_queries_are_single_flight() {
    let dir = temp_path("cache");
    let socket = dir.join("tailscaled.socket");
    let listener = UnixListener::bind(&socket).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let ready = Arc::new(Notify::new());
    let server = tokio::spawn(serve(
        listener,
        vec![tagged_whois(), tagged_whois()],
        count.clone(),
        ready.clone(),
    ));
    ready.notified().await;
    let clock = FakeClock::new();
    let source = Arc::new(ChainedWhoIs::with_clock(
        Arc::new(LocalApiSource::new(&socket)),
        Arc::new(CliSource::new(dir.join("missing"))),
        clock.clone(),
    ));
    let peer: SocketAddr = "100.64.0.2:7400".parse().unwrap();
    let mut jobs = Vec::new();
    for _ in 0..10 {
        let source = source.clone();
        jobs.push(tokio::spawn(async move {
            source.whois(peer).await.unwrap();
        }));
    }
    for job in jobs {
        job.await.unwrap();
    }
    assert_eq!(count.load(Ordering::Relaxed), 1);
    assert!(source.whois(peer).await.is_ok());
    clock.advance(30_000_001);
    assert!(source.whois(peer).await.is_ok());
    assert_eq!(count.load(Ordering::Relaxed), 2);
    server.await.unwrap();
}

#[tokio::test]
async fn cli_timeout_returns_and_kills_child() {
    let dir = temp_path("timeout");
    let executable = fake_tailscale(&dir, &whois_json(), Some(10));
    let started = std::time::Instant::now();
    let result = CliSource::new(executable)
        .whois("100.64.0.2:7400".parse().unwrap())
        .await;
    assert!(matches!(result, Err(WhoIsError::Timeout)));
    assert!(started.elapsed() < Duration::from_secs(5));
}
