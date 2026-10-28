//! One request per connection over HTTP/1.1. TLS is rustls with the ring
//! provider (no aws-lc-rs on iOS) and the Mozilla roots compiled in by
//! webpki-roots; certificates and host names are always verified.

use std::{
    pin::pin,
    sync::{Arc, OnceLock},
    time::Duration,
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    header::{ACCEPT, HOST, USER_AGENT},
    http::request::Builder,
    Method, Request, StatusCode,
};
use hyper_util::rt::TokioIo;
use rustls::{pki_types::ServerName, ClientConfig, RootCertStore};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};
use tokio_rustls::TlsConnector;
use url::{Host, Url};

/// Connect, TLS, request and response body together.
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY_BYTES: usize = 16 << 20;

/// `scheme://host[:port]`; API paths are relative to it.
#[derive(Clone)]
pub(super) struct Base {
    tls: bool,
    host: String,
    port: u16,
    authority: String,
}

impl Base {
    pub(super) fn parse(base: &str) -> Option<Self> {
        let url = Url::parse(base).ok()?;
        let tls = match url.scheme() {
            "https" => true,
            "http" => false,
            _ => return None,
        };
        let host = match url.host()? {
            Host::Domain(domain) => domain.to_owned(),
            Host::Ipv4(ip) => ip.to_string(),
            Host::Ipv6(ip) => ip.to_string(),
        };
        let authority = match url.port() {
            Some(port) => format!("{}:{port}", url.host_str()?),
            None => url.host_str()?.to_owned(),
        };
        Some(Self {
            tls,
            host,
            port: url.port_or_known_default()?,
            authority,
        })
    }

    /// Keep this Host header but connect where `dial` points (tests).
    pub(super) fn dialing(self, dial: &Base) -> Self {
        Self {
            tls: dial.tls,
            host: dial.host.clone(),
            port: dial.port,
            authority: self.authority,
        }
    }

    /// `path` is origin-form (`/api/v2/…?…`).
    pub(super) fn request(&self, method: Method, path: &str) -> Builder {
        Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, &self.authority)
            .header(USER_AGENT, "remote-screen")
            .header(ACCEPT, "application/json")
    }
}

/// Errors are transport failures only (connect, TLS, timeout, reset); their
/// text never contains request headers or bodies.
pub(super) async fn send(
    base: &Base,
    request: Request<Full<Bytes>>,
) -> Result<(StatusCode, Bytes), String> {
    tokio::time::timeout(TIMEOUT, exchange(base, request))
        .await
        .unwrap_or_else(|_| Err("timed out".to_owned()))
}

async fn exchange(
    base: &Base,
    request: Request<Full<Bytes>>,
) -> Result<(StatusCode, Bytes), String> {
    let tcp = TcpStream::connect((base.host.as_str(), base.port))
        .await
        .map_err(|error| format!("connect: {error}"))?;
    if !base.tls {
        return roundtrip(tcp, request).await;
    }
    let name = ServerName::try_from(base.host.clone()).map_err(|_| "bad host name".to_owned())?;
    let config = tls_config().map_err(|error| format!("tls: {error}"))?;
    let tls = TlsConnector::from(config)
        .connect(name, tcp)
        .await
        .map_err(|error| format!("tls: {error}"))?;
    roundtrip(tls, request).await
}

/// Drive the connection in this future (no spawned task), so a timeout drops
/// the socket with it.
async fn roundtrip<S>(io: S, request: Request<Full<Bytes>>) -> Result<(StatusCode, Bytes), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .map_err(|error| format!("http: {error}"))?;
    let mut response = pin!(async move {
        let response = sender
            .send_request(request)
            .await
            .map_err(|error| format!("http: {error}"))?;
        let status = response.status();
        let body = Limited::new(response.into_body(), MAX_BODY_BYTES)
            .collect()
            .await
            .map_err(|error| format!("http body: {error}"))?;
        Ok::<_, String>((status, body.to_bytes()))
    });
    let mut conn = pin!(conn);
    tokio::select! {
        result = &mut response => result,
        done = &mut conn => match done {
            Ok(()) => response.await,
            Err(error) => Err(format!("http: {error}")),
        },
    }
}

fn tls_config() -> Result<Arc<ClientConfig>, rustls::Error> {
    static CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    if let Some(config) = CONFIG.get() {
        return Ok(config.clone());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        })
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(CONFIG.get_or_init(|| Arc::new(config)).clone())
}
