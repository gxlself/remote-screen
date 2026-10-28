//! Tailscale API calls (the OAuth client-credentials exchange and the device
//! list) and the Headscale node list. Logs carry only the token kind, HTTP
//! status and elapsed time.

use std::time::Instant;

use bytes::Bytes;
use http_body_util::Full;
use hyper::{
    header::{AUTHORIZATION, CONTENT_TYPE},
    http::HeaderValue,
    Method, Request,
};
use serde::{Deserialize, Deserializer};
use tracing::{info, warn};
use zeroize::Zeroizing;

use super::dto::{
    map_devices, map_nodes, ApiDevices, HeadscaleNodes, TailscaleApiError, TailscaleDevice,
};
use super::http::{send, Base};
use super::token_chars_ok;
use crate::rscore::error::CoreError;

/// One failed API call. No variant holds request data or response text.
#[derive(Debug)]
pub(super) enum Fail {
    /// 401 / 403.
    Auth(u16),
    /// Any other non-2xx status.
    Status(u16),
    Network(String),
    /// Unusable 2xx body (serde's category and position only: the token
    /// response carries the access token), or a request that cannot be built.
    Invalid(String),
}

impl Fail {
    pub(super) fn kind(&self) -> TailscaleApiError {
        match self {
            Self::Auth(_) => TailscaleApiError::Auth,
            Self::Network(_) => TailscaleApiError::Network,
            Self::Status(_) | Self::Invalid(_) => TailscaleApiError::Http,
        }
    }
}

impl From<Fail> for CoreError {
    fn from(fail: Fail) -> Self {
        match fail {
            Fail::Auth(status) => Self::Http {
                status,
                hub_code: None,
                message: "tailscale api unauthorized".into(),
            },
            Fail::Status(status) => Self::Http {
                status,
                hub_code: None,
                message: "tailscale api request failed".into(),
            },
            Fail::Network(message) => Self::network(format!("tailscale api: {message}")),
            Fail::Invalid(message) => Self::internal(format!("tailscale api: {message}")),
        }
    }
}

pub(super) struct AccessToken {
    pub(super) token: Zeroizing<String>,
    pub(super) expires_in: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(deserialize_with = "deserialize_zeroizing")]
    access_token: Zeroizing<String>,
    #[serde(default)]
    expires_in: u64,
}

fn deserialize_zeroizing<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Zeroizing<String>, D::Error> {
    String::deserialize(deserializer).map(Zeroizing::new)
}

/// `POST /api/v2/oauth/token`. Tailscale ignores `client_id`; its CLI sends
/// the same placeholder.
pub(super) async fn oauth_token(base: &Base, secret: &str) -> Result<AccessToken, Fail> {
    let form = Zeroizing::new(
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "client_credentials")
            .append_pair("client_id", "some-client-id")
            .append_pair("client_secret", secret)
            .finish(),
    );
    let request = base
        .request(Method::POST, "/api/v2/oauth/token")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Full::new(Bytes::copy_from_slice(form.as_bytes())))
        .map_err(|_| Fail::Invalid("bad request".into()))?;
    let body = call(base, "client", request).await?;
    let response: TokenResponse = serde_json::from_slice(&body).map_err(json_fail)?;
    if !token_chars_ok(&response.access_token) {
        return Err(Fail::Invalid("bad access token".into()));
    }
    Ok(AccessToken {
        token: response.access_token,
        expires_in: response.expires_in,
    })
}

/// `GET /api/v2/tailnet/-/devices?fields=all`, mapped and sorted.
pub(super) async fn devices(
    base: &Base,
    kind: &'static str,
    bearer: &str,
) -> Result<Vec<TailscaleDevice>, Fail> {
    let body = get(base, kind, bearer, "/api/v2/tailnet/-/devices?fields=all").await?;
    let devices: ApiDevices = serde_json::from_slice(&body).map_err(json_fail)?;
    Ok(map_devices(devices))
}

/// Headscale `GET /api/v1/node` with an API key, mapped and sorted.
pub(super) async fn nodes(base: &Base, key: &str) -> Result<Vec<TailscaleDevice>, Fail> {
    let body = get(base, "headscale", key, "/api/v1/node").await?;
    let nodes: HeadscaleNodes = serde_json::from_slice(&body).map_err(json_fail)?;
    Ok(map_nodes(nodes))
}

async fn get(base: &Base, kind: &'static str, bearer: &str, path: &str) -> Result<Bytes, Fail> {
    let mut authorization = HeaderValue::from_str(&Zeroizing::new(format!("Bearer {bearer}")))
        .map_err(|_| Fail::Invalid("bad request".into()))?;
    authorization.set_sensitive(true);
    let request = base
        .request(Method::GET, path)
        .header(AUTHORIZATION, authorization)
        .body(Full::new(Bytes::new()))
        .map_err(|_| Fail::Invalid("bad request".into()))?;
    call(base, kind, request).await
}

async fn call(
    base: &Base,
    kind: &'static str,
    request: Request<Full<Bytes>>,
) -> Result<Bytes, Fail> {
    let started = Instant::now();
    let result = send(base, request).await;
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match result {
        Ok((status, body)) => {
            let status = status.as_u16();
            info!(token = kind, status, elapsed_ms, "tailscale api response");
            match status {
                200..=299 => Ok(body),
                401 | 403 => Err(Fail::Auth(status)),
                _ => Err(Fail::Status(status)),
            }
        }
        Err(message) => {
            warn!(token = kind, elapsed_ms, "tailscale api unreachable");
            Err(Fail::Network(message))
        }
    }
}

fn json_fail(error: serde_json::Error) -> Fail {
    Fail::Invalid(format!(
        "bad json ({:?} at line {} column {})",
        error.classify(),
        error.line(),
        error.column()
    ))
}
