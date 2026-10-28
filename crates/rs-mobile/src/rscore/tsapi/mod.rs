//! 10-24 Tailscale API token: list tailnet devices directly.
//!
//! The token lives only in the native SecretStore (`tailscale.api.token`).
//! For the official API, `tskey-client-…` is an OAuth client secret
//! (exchanged for a short-lived access token kept in memory) and anything else
//! is used as a Bearer API token. With a Headscale control server (saved
//! beside it as `tailscale.api.control`), the token is a Headscale API key.
//! Device lists are cached in memory for 30 s.

mod client;
pub mod dto;
mod http;
#[cfg(test)]
mod tests;

use std::{
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use tracing::{info, warn};
use url::Url;
use zeroize::Zeroizing;

use self::client::Fail;
use self::dto::{TailscaleApiError, TailscaleApiStatus, TailscaleDevice};
use self::http::Base;
use super::adapters::secrets::{SecretError, SecretStore};
use super::error::CoreError;

pub const TOKEN_KEY: &str = "tailscale.api.token";
/// `https://host[:port]` of a Headscale server; written and deleted with the
/// token, absent for the official API.
pub const CONTROL_KEY: &str = "tailscale.api.control";
const PRODUCTION_BASE: &str = "https://api.tailscale.com";
const MAX_TOKEN_BYTES: usize = 512;
const CACHE_TTL: Duration = Duration::from_secs(30);
/// Exchange the OAuth access token again this long before it expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// Monotonic time source for the cache and access token expiry.
pub(crate) type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

pub struct TsApi {
    base: Base,
    /// Tests: Headscale requests connect here instead of the control server.
    control_dial: Option<Base>,
    clock: Clock,
    state: Mutex<State>,
    /// Serializes set/clear/devices so concurrent callers share one request.
    ops: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    cache: Option<(Instant, Vec<TailscaleDevice>)>,
    access: Option<CachedAccess>,
    last_error: Option<TailscaleApiError>,
    last_ok_at: Option<u64>,
    device_count: Option<usize>,
}

struct CachedAccess {
    /// The client secret this access token was issued for.
    secret: Zeroizing<String>,
    token: Zeroizing<String>,
    refresh_at: Instant,
}

enum Token {
    Api(Zeroizing<String>),
    Client(Zeroizing<String>),
    /// An API key and the control server it belongs to.
    Headscale(Zeroizing<String>, String),
}

impl Token {
    /// Trim, then reject empty, over-long and non-printable-ASCII tokens.
    /// `control` is an already checked Headscale server.
    fn parse(raw: &str, control: Option<String>) -> Result<Self, CoreError> {
        let token = raw.trim();
        if token.len() > MAX_TOKEN_BYTES || !token_chars_ok(token) {
            return Err(CoreError::internal("bad token"));
        }
        let token = Zeroizing::new(token.to_owned());
        Ok(match control {
            Some(control) => Self::Headscale(token, control),
            None if token.starts_with("tskey-client-") => Self::Client(token),
            None => Self::Api(token),
        })
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::Api(_) => "api",
            Self::Client(_) => "client",
            Self::Headscale(..) => "headscale",
        }
    }

    fn secret(&self) -> &str {
        match self {
            Self::Api(token) | Self::Client(token) | Self::Headscale(token, _) => token,
        }
    }

    fn control(&self) -> Option<&str> {
        match self {
            Self::Headscale(_, control) => Some(control),
            Self::Api(_) | Self::Client(_) => None,
        }
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Token({}, <redacted>)", self.kind())
    }
}

/// Non-empty, and only visible ASCII (no whitespace or control characters).
fn token_chars_ok(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|byte| byte.is_ascii_graphic())
}

/// `None` (also for an empty or blank string) means the official API.
/// Otherwise only `https://host[:port]` is accepted, with at most a trailing
/// `/`; it comes back normalized (host lowercased, default port and `/`
/// dropped).
fn parse_control_url(raw: Option<&str>) -> Result<Option<String>, CoreError> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let bad = || CoreError::internal("bad control url");
    let url = Url::parse(raw).map_err(|_| bad())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(bad());
    }
    let host = url
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(bad)?;
    Ok(Some(match url.port() {
        Some(port) => format!("https://{host}:{port}"),
        None => format!("https://{host}"),
    }))
}

impl TsApi {
    pub fn production() -> Self {
        let base = Base::parse(PRODUCTION_BASE).expect("production base is a valid https url");
        Self::new(base, None, Arc::new(Instant::now))
    }

    /// Tests point this at `http://127.0.0.1:<port>`. Headscale requests
    /// connect there too, still naming the control server in the Host header.
    #[cfg(test)]
    pub(crate) fn with_base(base: &str, clock: Clock) -> Result<Self, CoreError> {
        let base = Base::parse(base).ok_or_else(|| CoreError::internal("bad base url"))?;
        Ok(Self::new(base.clone(), Some(base), clock))
    }

    fn new(base: Base, control_dial: Option<Base>, clock: Clock) -> Self {
        Self {
            base,
            control_dial,
            clock,
            state: Mutex::default(),
            ops: tokio::sync::Mutex::new(()),
        }
    }

    /// List devices with `token`, from the Headscale server at `control_url`
    /// when one is given; only on success save both (replacing the old pair)
    /// and refresh the cache. On failure the stored values are untouched.
    pub async fn set_token(
        &self,
        store: &dyn SecretStore,
        token: &str,
        control_url: Option<&str>,
    ) -> Result<Vec<TailscaleDevice>, CoreError> {
        let token = Token::parse(token, parse_control_url(control_url)?)?;
        let _op = self.ops.lock().await;
        let devices = self.fetch(&token).await?;
        save(store, &token)?;
        self.state().cache = Some(((self.clock)(), devices.clone()));
        Ok(devices)
    }

    pub async fn clear_token(&self, store: &dyn SecretStore) -> Result<(), CoreError> {
        let _op = self.ops.lock().await;
        store.delete(TOKEN_KEY).map_err(store_error)?;
        store.delete(CONTROL_KEY).map_err(store_error)?;
        *self.state() = State::default();
        Ok(())
    }

    pub fn status(&self, store: &dyn SecretStore) -> Result<TailscaleApiStatus, CoreError> {
        let configured = store.get(TOKEN_KEY).map_err(store_error)?.is_some();
        let control_url = if configured {
            stored_control(store)?
        } else {
            None
        };
        let state = self.state();
        Ok(TailscaleApiStatus {
            configured,
            control_url,
            last_error: state.last_error,
            last_ok_at: state.last_ok_at,
            device_count: state.device_count,
        })
    }

    /// Cached for 30 s unless `force`.
    pub async fn devices(
        &self,
        store: &dyn SecretStore,
        force: bool,
    ) -> Result<Vec<TailscaleDevice>, CoreError> {
        let _op = self.ops.lock().await;
        let Some(stored) = store.get(TOKEN_KEY).map_err(store_error)? else {
            return Err(CoreError::unsupported("tailscale api not configured"));
        };
        let control = stored_control(store)?;
        let token = std::str::from_utf8(&stored)
            .map_err(|_| CoreError::internal("stored token is malformed"))
            .and_then(|token| Token::parse(token, control))?;
        if !force {
            let now = (self.clock)();
            if let Some((at, devices)) = &self.state().cache {
                if now.saturating_duration_since(*at) < CACHE_TTL {
                    return Ok(devices.clone());
                }
            }
        }
        let devices = self.fetch(&token).await?;
        self.state().cache = Some(((self.clock)(), devices.clone()));
        Ok(devices)
    }

    /// One listing; records the outcome in the status.
    async fn fetch(&self, token: &Token) -> Result<Vec<TailscaleDevice>, CoreError> {
        let result = match token {
            Token::Api(token) => client::devices(&self.base, "api", token).await,
            Token::Client(secret) => match self.access_token(secret).await {
                Ok(access) => client::devices(&self.base, "client", &access).await,
                Err(fail) => Err(fail),
            },
            Token::Headscale(key, control) => match self.control_base(control) {
                Some(base) => client::nodes(&base, key).await,
                None => Err(Fail::Invalid("bad control url".into())),
            },
        };
        let mut state = self.state();
        match result {
            Ok(devices) => {
                info!(
                    token = token.kind(),
                    devices = devices.len(),
                    "tailscale api devices"
                );
                state.last_error = None;
                state.last_ok_at = Some(now_us());
                state.device_count = Some(devices.len());
                Ok(devices)
            }
            Err(fail) => {
                warn!(token = token.kind(), error = ?fail.kind(), "tailscale api failed");
                state.last_error = Some(fail.kind());
                Err(fail.into())
            }
        }
    }

    /// Reuse the in-memory access token for this secret until `REFRESH_MARGIN`
    /// before it expires.
    async fn access_token(&self, secret: &Zeroizing<String>) -> Result<Zeroizing<String>, Fail> {
        let now = (self.clock)();
        if let Some(access) = &self.state().access {
            if access.secret == *secret && now < access.refresh_at {
                return Ok(access.token.clone());
            }
        }
        let issued = client::oauth_token(&self.base, secret).await?;
        let lifetime = Duration::from_secs(issued.expires_in).saturating_sub(REFRESH_MARGIN);
        self.state().access = Some(CachedAccess {
            secret: secret.clone(),
            token: issued.token.clone(),
            refresh_at: now.checked_add(lifetime).unwrap_or(now),
        });
        Ok(issued.token)
    }

    fn control_base(&self, control: &str) -> Option<Base> {
        let base = Base::parse(control)?;
        Some(match &self.control_dial {
            Some(dial) => base.dialing(dial),
            None => base,
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// The token, then its control server (or no stale one). When the second
/// write fails the token goes too, so a key is never left paired with another
/// server.
fn save(store: &dyn SecretStore, token: &Token) -> Result<(), CoreError> {
    store
        .put(TOKEN_KEY, token.secret().as_bytes())
        .map_err(store_error)?;
    let control = match token.control() {
        Some(control) => store.put(CONTROL_KEY, control.as_bytes()),
        None => store.delete(CONTROL_KEY),
    };
    control.map_err(|error| {
        let _ = store.delete(TOKEN_KEY);
        store_error(error)
    })
}

/// The saved Headscale server, checked again; `None` for the official API.
fn stored_control(store: &dyn SecretStore) -> Result<Option<String>, CoreError> {
    let Some(stored) = store.get(CONTROL_KEY).map_err(store_error)? else {
        return Ok(None);
    };
    let control =
        std::str::from_utf8(&stored).map_err(|_| CoreError::internal("bad control url"))?;
    parse_control_url(Some(control))
}

fn store_error(error: SecretError) -> CoreError {
    CoreError::internal(format!("secret store: {error}"))
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
