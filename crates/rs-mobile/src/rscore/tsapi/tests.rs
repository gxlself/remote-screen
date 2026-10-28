use std::{
    collections::HashMap,
    io::Write,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use rustls::pki_types::PrivatePkcs8KeyDer;
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use super::dto::{TailscaleApiError, TailscaleApiStatus, TailscaleDevice, TailscaleOs};
use super::{Clock, Token, TsApi, CONTROL_KEY, TOKEN_KEY};
use crate::rscore::adapters::secrets::{MemSecretStore, SecretStore};
use crate::rscore::error::CoreError;

const API_TOKEN: &str = "tskey-api-TEST";
const CLIENT_SECRET: &str = "tskey-client-TEST";
const DEVICES_PATH: &str = "/api/v2/tailnet/-/devices?fields=all";
const TOKEN_PATH: &str = "/api/v2/oauth/token";
const HEADSCALE: &str = "https://hs.example.com";
const HEADSCALE_KEY: &str = "hskey-api-TEST.secretpart";
const NODES_PATH: &str = "/api/v1/node";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

type Handler = dyn Fn(&Seen) -> (u16, String) + Send + Sync;

/// Local HTTP/1.1 server: records each request and answers it with
/// `handler`'s status and JSON body, one request per connection.
struct Fake {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    async fn start(handler: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let handler: Arc<Handler> = Arc::new(handler);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (log, handler) = (log.clone(), handler.clone());
                tokio::spawn(async move { serve(stream, handler.as_ref(), &log).await });
            }
        });
        Self { addr, seen }
    }

    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

async fn serve(mut stream: TcpStream, handler: &Handler, log: &Mutex<Vec<Seen>>) {
    let mut buf = Vec::new();
    let mut chunk = [0; 4096];
    let head_end = loop {
        if let Some(at) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8(buf[..head_end].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap().split(' ');
    let method = request_line.next().unwrap().to_owned();
    let path = request_line.next().unwrap().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (key, value) = line.split_once(':').unwrap();
            (key.trim().to_owned(), value.trim().to_owned())
        })
        .collect();
    let length = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .map_or(0, |(_, value)| value.parse::<usize>().unwrap());
    while buf.len() < head_end + length {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let seen = Seen {
        method,
        path,
        headers,
        body: String::from_utf8(buf[head_end..head_end + length].to_vec()).unwrap(),
    };
    let (status, body) = handler(&seen);
    log.lock().unwrap().push(seen);
    let response = format!(
        "HTTP/1.1 {status} Fake\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Manually advanced monotonic clock.
#[derive(Clone)]
struct TestClock {
    start: Instant,
    secs: Arc<AtomicU64>,
}

impl TestClock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            secs: Arc::new(AtomicU64::new(0)),
        }
    }

    fn clock(&self) -> Clock {
        let clock = self.clone();
        Arc::new(move || clock.start + Duration::from_secs(clock.secs.load(Ordering::SeqCst)))
    }

    fn advance(&self, secs: u64) {
        self.secs.fetch_add(secs, Ordering::SeqCst);
    }
}

fn new_api(base: &str, clock: &TestClock) -> TsApi {
    TsApi::with_base(base, clock.clock()).unwrap()
}

fn devices_body() -> String {
    json!({"devices": [
        {
            "id": "1001",
            "nodeId": "nBeta",
            "name": "beta.tail1234.ts.net.",
            "hostname": "beta-host",
            "os": "macOS",
            "addresses": ["fd7a:115c:a1e0::2", "100.64.0.2"],
            "connectedToControl": true,
            "lastSeen": "2026-10-24T01:02:03Z",
            "tags": ["tag:server"]
        },
        {"id": "1002", "name": "", "hostname": "Alpha", "os": "LINUX", "addresses": ["100.64.0.1"]},
        {
            "nodeId": "nGamma",
            "name": "gamma.tail1234.ts.net",
            "os": "iPadOS",
            "addresses": [],
            "connectedToControl": false,
            "lastSeen": ""
        },
        {
            "nodeId": "nDelta",
            "name": "Delta.tail1234.ts.net.",
            "os": "tvOS",
            "addresses": ["fd7a:115c:a1e0::4", "100.64.0.4", "fd7a:115c:a1e0::5", "100.64.0.5"],
            "lastSeen": "2026-10-23T22:10:00.5+08:00",
            "tags": []
        },
        {
            "nodeId": "nEpsilon",
            "name": "epsilon.tail1234.ts.net.",
            "os": "windows",
            "connectedToControl": true,
            "lastSeen": "yesterday"
        },
        {"nodeId": "nZeta", "name": "zeta.tail1234.ts.net.", "os": "Android", "addresses": ["100.64.0.6"]},
        {"nodeId": "nEta", "name": "eta.tail1234.ts.net.", "os": "ios"}
    ]})
    .to_string()
}

fn expected_devices() -> Value {
    json!([
        {
            "id": "1002", "name": "Alpha", "dnsName": "", "os": "linux",
            "addresses": ["100.64.0.1"], "online": null, "lastSeenAt": null, "tags": []
        },
        {
            "id": "nBeta", "name": "beta", "dnsName": "beta.tail1234.ts.net", "os": "macos",
            "addresses": ["100.64.0.2", "fd7a:115c:a1e0::2"], "online": true,
            "lastSeenAt": 1_792_803_723_000_000_u64, "tags": ["tag:server"]
        },
        {
            "id": "nDelta", "name": "Delta", "dnsName": "Delta.tail1234.ts.net", "os": "other",
            "addresses": ["100.64.0.4", "100.64.0.5", "fd7a:115c:a1e0::4", "fd7a:115c:a1e0::5"],
            "online": null, "lastSeenAt": 1_792_764_600_500_000_u64, "tags": []
        },
        {
            "id": "nEpsilon", "name": "epsilon", "dnsName": "epsilon.tail1234.ts.net", "os": "windows",
            "addresses": [], "online": true, "lastSeenAt": null, "tags": []
        },
        {
            "id": "nEta", "name": "eta", "dnsName": "eta.tail1234.ts.net", "os": "ios",
            "addresses": [], "online": null, "lastSeenAt": null, "tags": []
        },
        {
            "id": "nGamma", "name": "gamma", "dnsName": "gamma.tail1234.ts.net", "os": "ios",
            "addresses": [], "online": false, "lastSeenAt": null, "tags": []
        },
        {
            "id": "nZeta", "name": "zeta", "dnsName": "zeta.tail1234.ts.net", "os": "android",
            "addresses": ["100.64.0.6"], "online": null, "lastSeenAt": null, "tags": []
        }
    ])
}

fn unauthorized(status: u16) -> CoreError {
    CoreError::Http {
        status,
        hub_code: None,
        message: "tailscale api unauthorized".into(),
    }
}

fn assert_no_secret(error: &CoreError, secret: &str) {
    assert!(!error.to_string().contains(secret), "{error}");
    assert!(!format!("{error:?}").contains(secret), "{error:?}");
}

/// OAuth fake: each token request issues `at-<n>` for an hour; device
/// requests need the latest one.
fn oauth_handler(issued: Arc<AtomicU64>) -> impl Fn(&Seen) -> (u16, String) + Send + Sync {
    move |seen| {
        if seen.path == TOKEN_PATH {
            let n = issued.fetch_add(1, Ordering::SeqCst) + 1;
            let body = json!({
                "access_token": format!("at-{n}"),
                "token_type": "Bearer",
                "expires_in": 3600,
                "scope": "devices:core:read"
            });
            return (200, body.to_string());
        }
        let bearer = format!("Bearer at-{}", issued.load(Ordering::SeqCst));
        if seen.header("authorization") == Some(bearer.as_str()) {
            (200, devices_body())
        } else {
            (401, "{}".into())
        }
    }
}

#[tokio::test]
async fn api_token_lists_devices_with_bearer() {
    let fake = Fake::start(|_| (200, devices_body())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();

    let devices = api
        .set_token(store.as_ref(), &format!("  {API_TOKEN}\n"), None)
        .await
        .unwrap();

    assert_eq!(serde_json::to_value(&devices).unwrap(), expected_devices());
    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].path, DEVICES_PATH);
    assert_eq!(
        seen[0].header("authorization"),
        Some("Bearer tskey-api-TEST")
    );
    assert_eq!(seen[0].header("host"), Some(fake.addr.to_string().as_str()));
    assert_eq!(store.raw(TOKEN_KEY).as_deref(), Some(API_TOKEN.as_bytes()));
    let status = api.status(store.as_ref()).unwrap();
    assert!(status.configured);
    assert_eq!(status.last_error, None);
    assert_eq!(status.device_count, Some(7));
    assert!(status.last_ok_at.is_some());
}

#[tokio::test]
async fn client_secret_exchanges_then_reuses_access_token() {
    let issued = Arc::new(AtomicU64::new(0));
    let fake = Fake::start(oauth_handler(issued.clone())).await;
    let clock = TestClock::new();
    let api = new_api(&fake.base(), &clock);
    let store = MemSecretStore::new();

    let devices = api
        .set_token(store.as_ref(), CLIENT_SECRET, None)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&devices).unwrap(), expected_devices());
    let seen = fake.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].path, TOKEN_PATH);
    assert_eq!(
        seen[0].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    let form: HashMap<String, String> = url::form_urlencoded::parse(seen[0].body.as_bytes())
        .into_owned()
        .collect();
    let expected: HashMap<String, String> = [
        ("grant_type", "client_credentials"),
        ("client_id", "some-client-id"),
        ("client_secret", CLIENT_SECRET),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect();
    assert_eq!(form, expected);
    assert_eq!(seen[1].method, "GET");
    assert_eq!(seen[1].path, DEVICES_PATH);
    assert_eq!(seen[1].header("authorization"), Some("Bearer at-1"));
    assert_eq!(
        store.raw(TOKEN_KEY).as_deref(),
        Some(CLIENT_SECRET.as_bytes())
    );

    // 3539 s into a 3600 s token: still more than 60 s left, no new exchange.
    clock.advance(3539);
    api.devices(store.as_ref(), true).await.unwrap();
    let seen = fake.seen();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[2].path, DEVICES_PATH);
    assert_eq!(seen[2].header("authorization"), Some("Bearer at-1"));

    // 3541 s: inside the last minute, exchange again.
    clock.advance(2);
    api.devices(store.as_ref(), true).await.unwrap();
    let seen = fake.seen();
    assert_eq!(seen.len(), 5);
    assert_eq!(seen[3].path, TOKEN_PATH);
    assert_eq!(seen[4].path, DEVICES_PATH);
    assert_eq!(seen[4].header("authorization"), Some("Bearer at-2"));
}

#[tokio::test]
async fn set_token_unauthorized_keeps_stored_token() {
    let fake = Fake::start(|_| (401, json!({"message": "invalid token"}).to_string())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();
    store.put(TOKEN_KEY, b"tskey-api-OLD").unwrap();

    let error = api
        .set_token(store.as_ref(), "tskey-api-NEW", None)
        .await
        .unwrap_err();

    assert_eq!(error, unauthorized(401));
    assert_eq!(store.raw(TOKEN_KEY).as_deref(), Some(&b"tskey-api-OLD"[..]));
    assert_eq!(store.put_count(), 1);
    assert_eq!(
        api.status(store.as_ref()).unwrap(),
        TailscaleApiStatus {
            configured: true,
            control_url: None,
            last_error: Some(TailscaleApiError::Auth),
            last_ok_at: None,
            device_count: None,
        }
    );
}

#[tokio::test]
async fn client_secret_rejections_are_auth() {
    // Token endpoint refuses the secret: no device request, nothing saved.
    let fake = Fake::start(|seen| {
        if seen.path == TOKEN_PATH {
            (403, "{}".into())
        } else {
            (200, devices_body())
        }
    })
    .await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();
    let error = api
        .set_token(store.as_ref(), CLIENT_SECRET, None)
        .await
        .unwrap_err();
    assert_eq!(error, unauthorized(403));
    assert_eq!(fake.seen().len(), 1);
    assert!(store.is_empty());
    let status = api.status(store.as_ref()).unwrap();
    assert_eq!(status.last_error, Some(TailscaleApiError::Auth));

    // Access token without the devices scope.
    let fake = Fake::start(|seen| {
        if seen.path == TOKEN_PATH {
            (
                200,
                json!({"access_token": "at-1", "expires_in": 3600}).to_string(),
            )
        } else {
            (403, "{}".into())
        }
    })
    .await;
    let api = new_api(&fake.base(), &TestClock::new());
    let error = api
        .set_token(store.as_ref(), CLIENT_SECRET, None)
        .await
        .unwrap_err();
    assert_eq!(error, unauthorized(403));
    assert_eq!(fake.seen().len(), 2);
    assert!(store.is_empty());
    let status = api.status(store.as_ref()).unwrap();
    assert_eq!(status.last_error, Some(TailscaleApiError::Auth));
}

#[tokio::test]
async fn devices_are_cached_for_thirty_seconds() {
    let fake = Fake::start(|_| (200, devices_body())).await;
    let clock = TestClock::new();
    let api = new_api(&fake.base(), &clock);
    let store = MemSecretStore::new();
    api.set_token(store.as_ref(), API_TOKEN, None)
        .await
        .unwrap();
    assert_eq!(fake.seen().len(), 1);

    let cached = api.devices(store.as_ref(), false).await.unwrap();
    assert_eq!(serde_json::to_value(&cached).unwrap(), expected_devices());
    clock.advance(29);
    api.devices(store.as_ref(), false).await.unwrap();
    assert_eq!(fake.seen().len(), 1);

    api.devices(store.as_ref(), true).await.unwrap();
    assert_eq!(fake.seen().len(), 2);

    // The forced request refreshed the cache at t=29 s.
    clock.advance(29);
    api.devices(store.as_ref(), false).await.unwrap();
    assert_eq!(fake.seen().len(), 2);
    clock.advance(1);
    api.devices(store.as_ref(), false).await.unwrap();
    assert_eq!(fake.seen().len(), 3);
}

#[tokio::test]
async fn closed_port_is_network_error() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let api = new_api(&format!("http://{addr}"), &TestClock::new());
    let store = MemSecretStore::new();

    let error = api
        .set_token(store.as_ref(), API_TOKEN, None)
        .await
        .unwrap_err();
    assert!(matches!(error, CoreError::Network { .. }), "{error:?}");
    assert_no_secret(&error, API_TOKEN);
    assert!(store.is_empty());
    assert_eq!(
        api.status(store.as_ref()).unwrap(),
        TailscaleApiStatus {
            configured: false,
            control_url: None,
            last_error: Some(TailscaleApiError::Network),
            last_ok_at: None,
            device_count: None,
        }
    );

    store.put(TOKEN_KEY, API_TOKEN.as_bytes()).unwrap();
    let error = api.devices(store.as_ref(), false).await.unwrap_err();
    assert!(matches!(error, CoreError::Network { .. }), "{error:?}");
    let status = api.status(store.as_ref()).unwrap();
    assert!(status.configured);
    assert_eq!(status.last_error, Some(TailscaleApiError::Network));
}

#[tokio::test]
async fn https_rejects_untrusted_certificate() {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec!["localhost".into()])
        .unwrap()
        .self_signed(&key)
        .unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handshakes = Arc::new(AtomicU64::new(0));
    let count = handshakes.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            if acceptor.accept(stream).await.is_ok() {
                count.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    let api = new_api(&format!("https://{addr}"), &TestClock::new());
    let store = MemSecretStore::new();

    let error = api
        .set_token(store.as_ref(), API_TOKEN, None)
        .await
        .unwrap_err();

    match &error {
        CoreError::Network { message } => assert!(message.contains("tls:"), "{message}"),
        other => panic!("expected a network error, got {other:?}"),
    }
    assert_eq!(handshakes.load(Ordering::SeqCst), 0);
    assert!(store.is_empty());
    let status = api.status(store.as_ref()).unwrap();
    assert_eq!(status.last_error, Some(TailscaleApiError::Network));
}

#[tokio::test]
async fn devices_without_token_is_unsupported() {
    let fake = Fake::start(|_| (200, devices_body())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();

    let error = api.devices(store.as_ref(), false).await.unwrap_err();

    assert_eq!(
        error,
        CoreError::Unsupported {
            message: "tailscale api not configured".into()
        }
    );
    assert!(fake.seen().is_empty());
    assert_eq!(
        api.status(store.as_ref()).unwrap(),
        TailscaleApiStatus::default()
    );
}

#[tokio::test]
async fn malformed_tokens_are_rejected_without_requests() {
    let fake = Fake::start(|_| (200, devices_body())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();
    let too_long = "a".repeat(513);

    for token in [
        "",
        " \t\n",
        "tskey-api-A B",
        "tskey-api-A\tB",
        "tskey-api-\u{7}",
        "tskey-api-\u{e9}",
        too_long.as_str(),
    ] {
        let error = api
            .set_token(store.as_ref(), token, None)
            .await
            .unwrap_err();
        assert_eq!(
            error,
            CoreError::Internal {
                message: "bad token".into()
            },
            "{token:?}"
        );
    }
    assert!(fake.seen().is_empty());
    assert!(store.is_empty());
    assert_eq!(
        api.status(store.as_ref()).unwrap(),
        TailscaleApiStatus::default()
    );

    api.set_token(store.as_ref(), &"a".repeat(512), None)
        .await
        .unwrap();
    assert_eq!(fake.seen().len(), 1);
}

#[tokio::test]
async fn http_failures_record_http_error() {
    let fake = Fake::start(|seen| match seen.header("authorization") {
        Some("Bearer tskey-api-FIVE") => (500, json!({"message": "boom"}).to_string()),
        Some("Bearer tskey-api-JUNK") => (200, "not json".into()),
        _ => (200, devices_body()),
    })
    .await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();

    let error = api
        .set_token(store.as_ref(), "tskey-api-FIVE", None)
        .await
        .unwrap_err();
    assert_eq!(
        error,
        CoreError::Http {
            status: 500,
            hub_code: None,
            message: "tailscale api request failed".into()
        }
    );
    let status = api.status(store.as_ref()).unwrap();
    assert_eq!(status.last_error, Some(TailscaleApiError::Http));

    let error = api
        .set_token(store.as_ref(), "tskey-api-JUNK", None)
        .await
        .unwrap_err();
    assert!(matches!(error, CoreError::Internal { .. }), "{error:?}");
    let status = api.status(store.as_ref()).unwrap();
    assert_eq!(status.last_error, Some(TailscaleApiError::Http));
    assert!(store.is_empty());

    api.set_token(store.as_ref(), API_TOKEN, None)
        .await
        .unwrap();
    let status = api.status(store.as_ref()).unwrap();
    assert_eq!(status.last_error, None);
    assert_eq!(status.device_count, Some(7));
}

#[tokio::test]
async fn errors_and_debug_never_contain_tokens() {
    for (code, body) in [
        (401, "{}"),
        (500, "{\"message\":\"boom\"}"),
        (200, "not json"),
    ] {
        let fake = Fake::start(move |_| (code, body.to_owned())).await;
        let api = new_api(&fake.base(), &TestClock::new());
        let store = MemSecretStore::new();
        for token in [API_TOKEN, CLIENT_SECRET] {
            let error = api
                .set_token(store.as_ref(), token, None)
                .await
                .unwrap_err();
            assert_no_secret(&error, token);
        }
    }

    // Neither the secret nor the access token appear when a later step fails,
    // including a token response that does not parse.
    for token_body in [
        json!({"access_token": "tskey-access-SECRET", "expires_in": 3600}).to_string(),
        r#"{"access_token": "tskey-access-SECRET", "expires_in": "soon"}"#.to_owned(),
    ] {
        let fake = Fake::start(move |seen| {
            if seen.path == TOKEN_PATH {
                (200, token_body.clone())
            } else {
                (500, "{}".into())
            }
        })
        .await;
        let api = new_api(&fake.base(), &TestClock::new());
        let store = MemSecretStore::new();
        let error = api
            .set_token(store.as_ref(), CLIENT_SECRET, None)
            .await
            .unwrap_err();
        assert_no_secret(&error, CLIENT_SECRET);
        assert_no_secret(&error, "tskey-access-SECRET");
        assert_no_secret(&error, "soon");
    }

    for token in [API_TOKEN, CLIENT_SECRET] {
        let parsed = Token::parse(token, None).unwrap();
        let debug = format!("{parsed:?}");
        assert!(!debug.contains(token), "{debug}");
    }
}

#[tokio::test]
async fn clear_token_resets_store_cache_and_access_token() {
    let issued = Arc::new(AtomicU64::new(0));
    let fake = Fake::start(oauth_handler(issued)).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();
    api.set_token(store.as_ref(), CLIENT_SECRET, None)
        .await
        .unwrap();
    assert_eq!(fake.seen().len(), 2);
    let deletes = store.delete_count();

    api.clear_token(store.as_ref()).await.unwrap();

    assert!(store.raw(TOKEN_KEY).is_none());
    assert_eq!(store.delete_count(), deletes + 2);
    assert_eq!(
        api.status(store.as_ref()).unwrap(),
        TailscaleApiStatus::default()
    );
    assert_eq!(
        api.devices(store.as_ref(), false).await.unwrap_err(),
        CoreError::Unsupported {
            message: "tailscale api not configured".into()
        }
    );

    // The in-memory access token went with it: the same secret exchanges again.
    api.set_token(store.as_ref(), CLIENT_SECRET, None)
        .await
        .unwrap();
    let seen = fake.seen();
    assert_eq!(seen.len(), 4);
    assert_eq!(seen[2].path, TOKEN_PATH);
    assert_eq!(seen[3].header("authorization"), Some("Bearer at-2"));
}

#[test]
fn dto_json_matches_types_ts() {
    let empty = TailscaleDevice {
        id: "n1".into(),
        name: "host".into(),
        dns_name: String::new(),
        os: TailscaleOs::Other,
        addresses: Vec::new(),
        online: None,
        last_seen_at: None,
        tags: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(&empty).unwrap(),
        json!({
            "id": "n1", "name": "host", "dnsName": "", "os": "other",
            "addresses": [], "online": null, "lastSeenAt": null, "tags": []
        })
    );
    let full = TailscaleDevice {
        id: "n2".into(),
        name: "aliyun-bj".into(),
        dns_name: "aliyun-bj.tail1234.ts.net".into(),
        os: TailscaleOs::Linux,
        addresses: vec!["100.64.0.1".into(), "fd7a:115c:a1e0::1".into()],
        online: Some(true),
        last_seen_at: Some(1_792_803_723_000_000),
        tags: vec!["tag:server".into()],
    };
    assert_eq!(
        serde_json::to_value(&full).unwrap(),
        json!({
            "id": "n2", "name": "aliyun-bj", "dnsName": "aliyun-bj.tail1234.ts.net", "os": "linux",
            "addresses": ["100.64.0.1", "fd7a:115c:a1e0::1"], "online": true,
            "lastSeenAt": 1_792_803_723_000_000_u64, "tags": ["tag:server"]
        })
    );
    for (os, text) in [
        (TailscaleOs::Linux, "linux"),
        (TailscaleOs::Macos, "macos"),
        (TailscaleOs::Windows, "windows"),
        (TailscaleOs::Ios, "ios"),
        (TailscaleOs::Android, "android"),
        (TailscaleOs::Other, "other"),
        (TailscaleOs::Unknown, "unknown"),
    ] {
        assert_eq!(serde_json::to_value(os).unwrap(), json!(text));
    }

    assert_eq!(
        serde_json::to_value(TailscaleApiStatus::default()).unwrap(),
        json!({
            "configured": false, "controlUrl": null, "lastError": null,
            "lastOkAt": null, "deviceCount": null
        })
    );
    for (error, text) in [
        (TailscaleApiError::Auth, "auth"),
        (TailscaleApiError::Network, "network"),
        (TailscaleApiError::Http, "http"),
    ] {
        let status = TailscaleApiStatus {
            configured: true,
            control_url: Some(HEADSCALE.into()),
            last_error: Some(error),
            last_ok_at: Some(5),
            device_count: Some(2),
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            json!({
                "configured": true, "controlUrl": "https://hs.example.com", "lastError": text,
                "lastOkAt": 5, "deviceCount": 2
            })
        );
    }
}

/// Log lines written on this thread while the guard lives. Tokio tests run
/// on one thread, so this includes the fake server.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        let logs = Self::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn nodes_body() -> String {
    json!({"nodes": [
        {
            "id": "7",
            "machineKey": "mkey:0001",
            "nodeKey": "nodekey:0001",
            "name": "studio-mac-raw",
            "givenName": "studio-mac",
            "user": {"id": "1", "name": "alice"},
            "ipAddresses": ["fd7a:115c:a1e0::7", "100.64.0.7"],
            "online": true,
            "lastSeen": "2026-10-24T01:02:03.000000123Z",
            "validTags": ["tag:server", "tag:office"],
            "forcedTags": ["tag:office", "tag:rs-host", "tag:office"],
            "invalidTags": ["tag:nope"],
            "registerMethod": "REGISTER_METHOD_AUTH_KEY"
        },
        {
            "id": 3,
            "name": "Alpha-box",
            "givenName": "",
            "ipAddresses": ["100.64.0.3"],
            "online": false,
            "lastSeen": "0001-01-01T00:00:00Z"
        },
        {"id": "9", "givenName": "gamma", "ipAddresses": null, "validTags": [], "forcedTags": null},
        {}
    ]})
    .to_string()
}

fn expected_nodes() -> Value {
    json!([
        {
            "id": "", "name": "", "dnsName": "", "os": "unknown",
            "addresses": [], "online": null, "lastSeenAt": null, "tags": []
        },
        {
            "id": "3", "name": "Alpha-box", "dnsName": "", "os": "unknown",
            "addresses": ["100.64.0.3"], "online": false, "lastSeenAt": null, "tags": []
        },
        {
            "id": "9", "name": "gamma", "dnsName": "", "os": "unknown",
            "addresses": [], "online": null, "lastSeenAt": null, "tags": []
        },
        {
            "id": "7", "name": "studio-mac", "dnsName": "", "os": "unknown",
            "addresses": ["100.64.0.7", "fd7a:115c:a1e0::7"], "online": true,
            "lastSeenAt": 1_792_803_723_000_000_u64,
            "tags": ["tag:server", "tag:office", "tag:rs-host"]
        }
    ])
}

fn bad_control_url() -> CoreError {
    CoreError::Internal {
        message: "bad control url".into(),
    }
}

#[tokio::test]
async fn headscale_lists_nodes_with_bearer() {
    let fake = Fake::start(|seen| {
        if seen.path == NODES_PATH {
            (200, nodes_body())
        } else {
            (404, "{}".into())
        }
    })
    .await;
    let clock = TestClock::new();
    let api = new_api(&fake.base(), &clock);
    let store = MemSecretStore::new();
    let (logs, guard) = Logs::capture();

    let devices = api
        .set_token(
            store.as_ref(),
            &format!(" {HEADSCALE_KEY}\n"),
            Some(" https://hs.example.com/ "),
        )
        .await
        .unwrap();

    assert_eq!(serde_json::to_value(&devices).unwrap(), expected_nodes());
    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].path, NODES_PATH);
    assert_eq!(
        seen[0].header("authorization"),
        Some("Bearer hskey-api-TEST.secretpart")
    );
    assert_eq!(seen[0].header("host"), Some("hs.example.com"));
    assert_eq!(
        store.raw(TOKEN_KEY).as_deref(),
        Some(HEADSCALE_KEY.as_bytes())
    );
    assert_eq!(
        store.raw(CONTROL_KEY).as_deref(),
        Some(HEADSCALE.as_bytes())
    );
    let status = api.status(store.as_ref()).unwrap();
    assert!(status.configured);
    assert_eq!(status.control_url.as_deref(), Some(HEADSCALE));
    assert_eq!(status.last_error, None);
    assert_eq!(status.device_count, Some(4));
    assert!(status.last_ok_at.is_some());

    // Later listings read the control server back from the store.
    clock.advance(29);
    api.devices(store.as_ref(), false).await.unwrap();
    assert_eq!(fake.seen().len(), 1);
    let listed = api.devices(store.as_ref(), true).await.unwrap();
    assert_eq!(serde_json::to_value(&listed).unwrap(), expected_nodes());
    let seen = fake.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].path, NODES_PATH);
    assert_eq!(
        seen[1].header("authorization"),
        Some("Bearer hskey-api-TEST.secretpart")
    );
    drop(guard);

    let text = logs.text();
    assert!(text.contains("token=\"headscale\""), "{text}");
    assert!(!text.contains(HEADSCALE_KEY), "{text}");
}

#[tokio::test]
async fn headscale_control_url_is_normalized() {
    let fake = Fake::start(|_| (200, json!({"nodes": []}).to_string())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();

    for (input, saved, host) in [
        (HEADSCALE, HEADSCALE, "hs.example.com"),
        (
            "\thttps://HS.Example.com:8443/",
            "https://hs.example.com:8443",
            "hs.example.com:8443",
        ),
        ("https://hs.example.com:443", HEADSCALE, "hs.example.com"),
        ("https://100.64.0.9", "https://100.64.0.9", "100.64.0.9"),
    ] {
        let devices = api
            .set_token(store.as_ref(), HEADSCALE_KEY, Some(input))
            .await
            .unwrap();
        assert!(devices.is_empty());
        let seen = fake.seen();
        let last = seen.last().unwrap();
        assert_eq!(last.path, NODES_PATH, "{input}");
        assert_eq!(last.header("host"), Some(host), "{input}");
        assert_eq!(
            store.raw(CONTROL_KEY).as_deref(),
            Some(saved.as_bytes()),
            "{input}"
        );
        assert_eq!(
            api.status(store.as_ref()).unwrap().control_url.as_deref(),
            Some(saved)
        );
    }

    // Null, empty and blank all mean the official API.
    let fake = Fake::start(|_| (200, devices_body())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    for control in [None, Some(""), Some("  \n")] {
        let store = MemSecretStore::new();
        store.put(CONTROL_KEY, HEADSCALE.as_bytes()).unwrap();
        api.set_token(store.as_ref(), API_TOKEN, control)
            .await
            .unwrap();
        assert_eq!(fake.seen().last().unwrap().path, DEVICES_PATH);
        assert!(store.raw(CONTROL_KEY).is_none(), "{control:?}");
        assert_eq!(api.status(store.as_ref()).unwrap().control_url, None);
    }
}

#[tokio::test]
async fn bad_control_urls_are_rejected_without_requests() {
    let fake = Fake::start(|_| (200, nodes_body())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();

    for control in [
        "http://hs.example.com",
        "ftp://hs.example.com",
        "hs.example.com",
        "https://hs.example.com/api",
        "https://hs.example.com/web/",
        "https://hs.example.com?x=1",
        "https://hs.example.com/?",
        "https://hs.example.com#top",
        "https://admin@hs.example.com",
        "https://admin:pw@hs.example.com",
        "https://:pw@hs.example.com",
        "https://",
        "https:///",
        "https://:443",
        "https://hs example.com",
    ] {
        let error = api
            .set_token(store.as_ref(), HEADSCALE_KEY, Some(control))
            .await
            .unwrap_err();
        assert_eq!(error, bad_control_url(), "{control:?}");
        assert_no_secret(&error, HEADSCALE_KEY);
    }
    assert!(fake.seen().is_empty());
    assert!(store.is_empty());
    assert_eq!(
        api.status(store.as_ref()).unwrap(),
        TailscaleApiStatus::default()
    );
}

#[tokio::test]
async fn headscale_unauthorized_saves_nothing() {
    let fake = Fake::start(|_| (401, json!({"message": "Unauthorized"}).to_string())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();
    store.put(TOKEN_KEY, b"tskey-api-OLD").unwrap();

    let error = api
        .set_token(store.as_ref(), HEADSCALE_KEY, Some(HEADSCALE))
        .await
        .unwrap_err();

    assert_eq!(error, unauthorized(401));
    assert_no_secret(&error, HEADSCALE_KEY);
    assert_eq!(fake.seen()[0].path, NODES_PATH);
    assert_eq!(store.raw(TOKEN_KEY).as_deref(), Some(&b"tskey-api-OLD"[..]));
    assert!(store.raw(CONTROL_KEY).is_none());
    assert_eq!(store.put_count(), 1);
    assert_eq!(
        api.status(store.as_ref()).unwrap(),
        TailscaleApiStatus {
            configured: true,
            control_url: None,
            last_error: Some(TailscaleApiError::Auth),
            last_ok_at: None,
            device_count: None,
        }
    );

    let store = MemSecretStore::new();
    let fake = Fake::start(|_| (403, "{}".into())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let error = api
        .set_token(store.as_ref(), HEADSCALE_KEY, Some(HEADSCALE))
        .await
        .unwrap_err();
    assert_eq!(error, unauthorized(403));
    assert!(store.is_empty());
}

#[tokio::test]
async fn headscale_errors_never_contain_the_key() {
    for (code, body) in [
        (500, "{\"message\":\"boom\"}"),
        (200, "not json"),
        (200, "{\"nodes\": {}}"),
    ] {
        let fake = Fake::start(move |_| (code, body.to_owned())).await;
        let api = new_api(&fake.base(), &TestClock::new());
        let store = MemSecretStore::new();
        let (logs, guard) = Logs::capture();
        let error = api
            .set_token(store.as_ref(), HEADSCALE_KEY, Some(HEADSCALE))
            .await
            .unwrap_err();
        drop(guard);
        assert_no_secret(&error, HEADSCALE_KEY);
        assert!(!logs.text().contains(HEADSCALE_KEY));
        assert!(logs.text().contains("token=\"headscale\""));
        assert!(store.is_empty());
        let status = api.status(store.as_ref()).unwrap();
        assert_eq!(status.last_error, Some(TailscaleApiError::Http));
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let api = new_api(&format!("http://{addr}"), &TestClock::new());
    let store = MemSecretStore::new();
    let error = api
        .set_token(store.as_ref(), HEADSCALE_KEY, Some(HEADSCALE))
        .await
        .unwrap_err();
    assert!(matches!(error, CoreError::Network { .. }), "{error:?}");
    assert_no_secret(&error, HEADSCALE_KEY);
    assert!(store.is_empty());

    let parsed = Token::parse(HEADSCALE_KEY, Some(HEADSCALE.into())).unwrap();
    let debug = format!("{parsed:?}");
    assert_eq!(debug, "Token(headscale, <redacted>)");
}

#[tokio::test]
async fn switching_between_headscale_and_tailscale() {
    let fake = Fake::start(|seen| {
        if seen.path == NODES_PATH {
            (200, nodes_body())
        } else {
            (200, devices_body())
        }
    })
    .await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();

    api.set_token(store.as_ref(), HEADSCALE_KEY, Some(HEADSCALE))
        .await
        .unwrap();
    let devices = api
        .set_token(store.as_ref(), API_TOKEN, None)
        .await
        .unwrap();
    assert_eq!(serde_json::to_value(&devices).unwrap(), expected_devices());
    assert_eq!(fake.seen()[1].path, DEVICES_PATH);
    assert_eq!(store.raw(TOKEN_KEY).as_deref(), Some(API_TOKEN.as_bytes()));
    assert!(store.raw(CONTROL_KEY).is_none());
    assert_eq!(api.status(store.as_ref()).unwrap().control_url, None);
    api.devices(store.as_ref(), true).await.unwrap();
    assert_eq!(fake.seen()[2].path, DEVICES_PATH);

    // A tskey-client- string sent to Headscale is just a Bearer key.
    api.set_token(store.as_ref(), CLIENT_SECRET, Some(HEADSCALE))
        .await
        .unwrap();
    let seen = fake.seen();
    assert_eq!(seen.len(), 4);
    assert_eq!(seen[3].path, NODES_PATH);
    assert_eq!(
        seen[3].header("authorization"),
        Some("Bearer tskey-client-TEST")
    );
}

#[tokio::test]
async fn clear_token_deletes_token_and_control_url() {
    let fake = Fake::start(|_| (200, nodes_body())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();
    api.set_token(store.as_ref(), HEADSCALE_KEY, Some(HEADSCALE))
        .await
        .unwrap();
    assert_eq!(store.len(), 2);
    let deletes = store.delete_count();

    api.clear_token(store.as_ref()).await.unwrap();

    assert!(store.is_empty());
    assert_eq!(store.delete_count(), deletes + 2);
    assert_eq!(
        api.status(store.as_ref()).unwrap(),
        TailscaleApiStatus::default()
    );
    assert_eq!(
        api.devices(store.as_ref(), false).await.unwrap_err(),
        CoreError::Unsupported {
            message: "tailscale api not configured".into()
        }
    );
    assert_eq!(fake.seen().len(), 1);
}

#[tokio::test]
async fn stored_control_url_is_checked_again() {
    let fake = Fake::start(|_| (200, nodes_body())).await;
    let api = new_api(&fake.base(), &TestClock::new());
    let store = MemSecretStore::new();
    store.put(TOKEN_KEY, HEADSCALE_KEY.as_bytes()).unwrap();
    store.put(CONTROL_KEY, b"http://hs.example.com").unwrap();

    assert_eq!(
        api.devices(store.as_ref(), true).await.unwrap_err(),
        bad_control_url()
    );
    assert!(fake.seen().is_empty());

    store
        .put(CONTROL_KEY, b"https://hs.example.com:8443")
        .unwrap();
    api.devices(store.as_ref(), true).await.unwrap();
    assert_eq!(fake.seen()[0].header("host"), Some("hs.example.com:8443"));
    assert_eq!(
        api.status(store.as_ref()).unwrap().control_url.as_deref(),
        Some("https://hs.example.com:8443")
    );
}
