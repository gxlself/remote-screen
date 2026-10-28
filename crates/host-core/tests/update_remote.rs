use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_core::update::prepare::Downloader;
use host_core::update::remote::CurlDownloader;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-h16e-update-remote-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn ok_body() -> Vec<u8> {
    (0..300 * 1024)
        .map(|index| u8::try_from(index % 251).expect("byte range"))
        .collect()
}

struct TestServer {
    base_url: String,
    connections: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}

impl TestServer {
    async fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("test server address");
        let base_url = format!("http://{addr}");
        let connections = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let accepted_connections = Arc::clone(&connections);
        let recorded_requests = Arc::clone(&requests);
        let redirect_base = base_url.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                accepted_connections.fetch_add(1, Ordering::SeqCst);
                let requests = Arc::clone(&recorded_requests);
                let redirect_base = redirect_base.clone();
                tokio::spawn(async move {
                    let _ = serve_connection(stream, requests, &redirect_base).await;
                });
            }
        });
        Self {
            base_url,
            connections,
            requests,
            task,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    requests: Arc<Mutex<Vec<String>>>,
    redirect_base: &str,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    loop {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if buffer.len() > 64 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request headers are too large",
            ));
        }
    }

    let request = String::from_utf8_lossy(&buffer);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    requests
        .lock()
        .expect("requests lock")
        .push(target.to_owned());
    let path = target.split('?').next().unwrap_or(target);

    match path {
        "/ok" => write_plain(&mut stream, "200 OK", &ok_body()).await,
        "/missing" => write_plain(&mut stream, "404 Not Found", b"missing").await,
        "/big" => write_plain(&mut stream, "200 OK", &vec![b'B'; 2048]).await,
        "/chunked" => write_chunked(&mut stream, &vec![b'C'; 2048]).await,
        "/redir" => {
            let location = format!("{redirect_base}/ok");
            let head = format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(head.as_bytes()).await?;
            stream.flush().await
        }
        _ => write_plain(&mut stream, "404 Not Found", b"unknown path").await,
    }
}

async fn write_plain(stream: &mut TcpStream, status: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

async fn write_chunked(stream: &mut TcpStream, body: &[u8]) -> std::io::Result<()> {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .await?;
    for chunk in body.chunks(512) {
        stream
            .write_all(format!("{:X}\r\n", chunk.len()).as_bytes())
            .await?;
        stream.write_all(chunk).await?;
        stream.write_all(b"\r\n").await?;
    }
    stream.write_all(b"0\r\n\r\n").await?;
    stream.flush().await
}

async fn curl_available() -> bool {
    tokio::process::Command::new("curl")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

async fn require_curl() -> Option<CurlDownloader> {
    if curl_available().await {
        Some(CurlDownloader::default())
    } else {
        eprintln!("curl not found, skipping");
        None
    }
}

#[tokio::test]
async fn downloads_package() {
    let Some(downloader) = require_curl().await else {
        return;
    };
    let server = TestServer::start().await;
    let dir = TestDir::new();
    let dest = dir.path().join("package.dmg");

    downloader
        .download(&server.url("/ok"), &dest, 1024 * 1024)
        .await
        .expect("download succeeds");

    assert_eq!(
        std::fs::metadata(&dest).expect("package metadata").len(),
        300 * 1024
    );
    assert_eq!(std::fs::read(dest).expect("read package"), ok_body());
}

#[tokio::test]
async fn missing_package_is_removed() {
    let Some(downloader) = require_curl().await else {
        return;
    };
    let server = TestServer::start().await;
    let dir = TestDir::new();
    let dest = dir.path().join("package.dmg");

    let error = downloader
        .download(&server.url("/missing"), &dest, 1024 * 1024)
        .await
        .expect_err("404 must fail");

    assert!(error.starts_with("curl exit 22"), "{error}");
    assert!(!dest.exists());
}

#[tokio::test]
async fn content_length_over_limit_is_removed() {
    let Some(downloader) = require_curl().await else {
        return;
    };
    let server = TestServer::start().await;
    let dir = TestDir::new();
    let dest = dir.path().join("package.dmg");

    let error = downloader
        .download(&server.url("/big"), &dest, 1024)
        .await
        .expect_err("oversized content length must fail");

    eprintln!("curl Content-Length over limit: {error}");
    assert!(error.starts_with("curl exit 63"), "{error}");
    assert_eq!(server.requests(), ["/big".to_owned()]);
    assert!(!dest.exists());
}

#[tokio::test]
async fn chunked_response_over_limit_is_removed() {
    let Some(downloader) = require_curl().await else {
        return;
    };
    let server = TestServer::start().await;
    let dir = TestDir::new();
    let dest = dir.path().join("package.dmg");

    let error = downloader
        .download(&server.url("/chunked"), &dest, 1024)
        .await
        .expect_err("oversized chunked response must fail");

    eprintln!("curl chunked over limit: {error}");
    assert!(
        error == "package too large" || error.starts_with("curl exit 63"),
        "unexpected curl error: {error}"
    );
    assert!(
        !server.requests().iter().any(|request| request == "/ok"),
        "chunked test must not redirect"
    );
    assert_eq!(server.requests(), ["/chunked".to_owned()]);
    assert!(!dest.exists());
}

#[tokio::test]
async fn follows_redirects() {
    let Some(downloader) = require_curl().await else {
        return;
    };
    let server = TestServer::start().await;
    let dir = TestDir::new();
    let dest = dir.path().join("package.dmg");

    downloader
        .download(&server.url("/redir"), &dest, 1024 * 1024)
        .await
        .expect("redirect succeeds");

    assert_eq!(std::fs::read(dest).expect("read package"), ok_body());
}

#[tokio::test]
async fn rejects_non_http_url_without_connecting() {
    let Some(_downloader) = require_curl().await else {
        return;
    };
    let downloader = CurlDownloader::default();
    let server = TestServer::start().await;
    let dir = TestDir::new();
    let dest = dir.path().join("package.dmg");
    let url = server.url("/x").replacen("http://", "ftp://", 1);

    let error = downloader
        .download(&url, &dest, 1024 * 1024)
        .await
        .expect_err("ftp must be rejected");

    assert_eq!(error, "unsupported url scheme");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(server.connections(), 0);
    assert!(!dest.exists());
}

#[tokio::test]
async fn reports_missing_curl() {
    let Some(_downloader) = require_curl().await else {
        return;
    };
    let dir = TestDir::new();
    let dest = dir.path().join("package.dmg");
    let downloader = CurlDownloader {
        program: PathBuf::from("/nonexistent/curl"),
        ..CurlDownloader::default()
    };

    let error = downloader
        .download("http://127.0.0.1:1/package.dmg", &dest, 1024)
        .await
        .expect_err("missing curl must fail");

    assert_eq!(error, "curl not found");
    assert!(!dest.exists());
}
