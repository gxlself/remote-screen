//! App-owned embedded tailnet lifecycle and status events.
//!
//! The Go tailnet wrapper is blocking, so all calls into it are dispatched to
//! Tokio's blocking pool.  The node itself is process-global in `rs-tailnet`,
//! while the lifecycle bookkeeping and event de-duplication belong to each
//! `Core`.

use std::net::IpAddr;
#[cfg(unix)]
use std::path::Path;
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::sync::{OnceLock, Weak};

#[cfg(unix)]
use rs_tailnet::{BackendState, Code, Tailnet, TailnetConfig, TailnetError, TailnetEvent};
use rsp_transport::net::NetDialer;
use serde::{Deserialize, Serialize};
use url::Url;

use super::error::CoreError;
#[cfg(unix)]
use super::runtime::core;
use super::runtime::Core;

#[cfg(not(unix))]
pub use unsupported::{available, login, logout, net_dialer, rebind, start, status_json, stop};

#[cfg(not(unix))]
fn unsupported_error() -> CoreError {
    CoreError::unsupported("tailnet is not available on this platform")
}

/// Options accepted by [`tailnet_start`].
///
/// This type intentionally does not implement `Debug`: an authorization key
/// must never be rendered by diagnostics or an error formatter.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TailnetStartOptions {
    pub control_url: String,
    pub hostname: String,
    #[serde(default)]
    pub auth_key: Option<String>,
    #[serde(default)]
    pub disable_netns: bool,
}

/// The event placed on RsCore's ordered event stream when the node changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailnetStateEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub state: String,
    pub auth_url: Option<String>,
    pub ips: Vec<IpAddr>,
    pub derp_region: String,
    pub message: Option<String>,
}

/// Per-Core tailnet bookkeeping.  The underlying Go node is process-global,
/// but each Core needs its own subscription state and last delivered event.
#[derive(Default)]
pub struct TailnetState(pub Mutex<TailnetStateInner>);

#[derive(Default)]
pub struct TailnetStateInner {
    #[cfg(unix)]
    pub tailnet: Option<Arc<Tailnet>>,
    pub started: bool,
    pub hook_registered: bool,
    pub last: Option<TailnetStateEvent>,
}

#[cfg(unix)]
static UNAVAILABLE_LOGGED: OnceLock<()> = OnceLock::new();

/// Return whether the embedded tailnet ABI can be loaded in this build.
#[cfg(unix)]
pub fn available() -> bool {
    match rs_tailnet::load_default() {
        Ok(_) => true,
        Err(error) => {
            if UNAVAILABLE_LOGGED.set(()).is_ok() {
                tracing::info!(error = ?error, "embedded tailnet is unavailable");
            }
            false
        }
    }
}

/// Parse and validate start options before checking whether the tailnet is
/// available.  This keeps malformed app input deterministic in every build.
pub fn parse_start_options(json: &str) -> Result<TailnetStartOptions, CoreError> {
    let mut opts: TailnetStartOptions = serde_json::from_str(json)
        .map_err(|error| CoreError::internal(format!("invalid tailnet options: {error}")))?;

    let url = Url::parse(&opts.control_url).map_err(|error| {
        CoreError::internal(format!(
            "invalid tailnet options: invalid controlUrl: {error}"
        ))
    })?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(CoreError::internal(
            "invalid tailnet options: controlUrl must be an http(s) URL with a host",
        ));
    }

    opts.hostname = opts.hostname.trim().to_owned();
    if opts.hostname.is_empty() {
        return Err(CoreError::internal(
            "invalid tailnet options: hostname must not be empty",
        ));
    }
    Ok(opts)
}

#[cfg(unix)]
fn map_error(error: TailnetError) -> CoreError {
    match error.code {
        Code::NotRunning | Code::Timeout | Code::Refused | Code::Unreachable | Code::Io => {
            CoreError::network(error.message)
        }
        Code::Invalid | Code::State | Code::Closed | Code::Unknown | Code::Ok => {
            CoreError::internal(error.message)
        }
    }
}

#[cfg(unix)]
fn map_error_redacted(error: TailnetError, secret: Option<&str>) -> CoreError {
    let message = redact_secret(error.message, secret);
    map_error(TailnetError {
        code: error.code,
        message,
    })
}

#[cfg(unix)]
fn redact_secret(mut message: String, secret: Option<&str>) -> String {
    if let Some(secret) = secret.filter(|secret| !secret.is_empty()) {
        message = message.replace(secret, "<redacted>");
    }
    message
}

#[cfg(unix)]
fn state_event(
    state: BackendState,
    auth_url: Option<String>,
    ips: Vec<IpAddr>,
    derp_region: String,
) -> TailnetStateEvent {
    TailnetStateEvent {
        event_type: "tailnet.state".to_owned(),
        state: state.as_camel().to_owned(),
        auth_url,
        ips,
        derp_region,
        message: None,
    }
}

#[cfg(unix)]
fn status_event(status: &rs_tailnet::TailnetStatus, auth_url: Option<String>) -> TailnetStateEvent {
    let (ips, derp_region) = status
        .self_node
        .as_ref()
        .map(|node| (node.ips.clone(), node.derp_region.clone()))
        .unwrap_or_default();
    state_event(
        status.state,
        auth_url.or_else(|| status.auth_url.clone()),
        ips,
        derp_region,
    )
}

#[cfg(unix)]
fn emit_state_event(core: &Weak<Core>, event: TailnetStateEvent) {
    let Some(core) = core.upgrade() else {
        return;
    };
    let state = core.state::<TailnetState>();
    let mut state = state.0.lock().unwrap_or_else(|error| error.into_inner());
    if state.last.as_ref() == Some(&event) {
        return;
    }
    state.last = Some(event.clone());
    // Keep the last-event comparison and enqueue operation under one lock so
    // concurrent producers cannot reorder two state transitions.
    core.events().emit(&event);
}

#[cfg(unix)]
fn event_callback(
    core: Weak<Core>,
    tailnet: Arc<Tailnet>,
) -> impl Fn(TailnetEvent) + Send + Sync + 'static {
    move |event| {
        if !matches!(event, TailnetEvent::Unknown) {
            if let Some(core) = core.upgrade() {
                super::tailnet_wire::node_changed(&core);
            }
        }
        match event {
            TailnetEvent::State { .. } => match tailnet.status() {
                Ok(status) => emit_state_event(&core, status_event(&status, None)),
                Err(error) => {
                    tracing::warn!(code = ?error.code, "tailnet status failed in event callback")
                }
            },
            TailnetEvent::Netmap => match tailnet.status() {
                Ok(status) => emit_state_event(&core, status_event(&status, None)),
                Err(error) => {
                    tracing::warn!(code = ?error.code, "tailnet status failed in event callback")
                }
            },
            TailnetEvent::AuthUrl { url } => match tailnet.status() {
                Ok(status) => {
                    emit_state_event(&core, status_event(&status, Some(url)));
                }
                Err(error) => {
                    tracing::warn!(code = ?error.code, "tailnet status failed in event callback")
                }
            },
            TailnetEvent::Error { message } => {
                let Some(core) = core.upgrade() else {
                    return;
                };
                let state = core.state::<TailnetState>();
                let state = state.0.lock().unwrap_or_else(|error| error.into_inner());
                let mut event = state.last.clone().unwrap_or_else(|| {
                    state_event(BackendState::Unknown, None, Vec::new(), String::new())
                });
                event.message = Some(message);
                core.events().emit(&event);
            }
            TailnetEvent::Stopped => {
                emit_state_event(
                    &core,
                    state_event(BackendState::Stopped, None, Vec::new(), String::new()),
                );
            }
            TailnetEvent::Unknown => {}
        }
    }
}

#[cfg(unix)]
fn ensure_directory(path: &Path) -> Result<(), CoreError> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(unix)]
async fn blocking<T, F>(core: &Arc<Core>, operation: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, TailnetError> + Send + 'static,
{
    core.handle()
        .spawn_blocking(operation)
        .await
        .map_err(CoreError::internal)?
        .map_err(map_error)
}

/// Start the process-global embedded node for one Core.
#[cfg(unix)]
pub async fn start(core: &Arc<Core>, opts: TailnetStartOptions) -> Result<(), CoreError> {
    let api = rs_tailnet::load_default()
        .map_err(|_| CoreError::unsupported("embedded tailnet is not available in this build"))?;
    let tailnet = Tailnet::global(api);
    let state = core.state::<TailnetState>();
    {
        let mut state = state.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.started {
            return Err(CoreError::internal("tailnet already started"));
        }
        state.tailnet = Some(tailnet.clone());
    }

    let dir = core.data_dir().join("tailnet");
    ensure_directory(&dir)?;
    let log_dir = core.log_dir();
    std::fs::create_dir_all(&log_dir)?;
    let config = TailnetConfig {
        dir,
        hostname: opts.hostname,
        control_url: opts.control_url,
        auth_key: opts.auth_key.clone(),
        ephemeral: false,
        log_file: Some(log_dir.join("tailnet.log")),
        verbose: false,
        disable_netns: opts.disable_netns,
        udp_idle_seconds: 0,
    };
    let callback = event_callback(Arc::downgrade(core), tailnet.clone());
    let secret = opts.auth_key.clone();
    let tailnet_for_start = tailnet.clone();
    let result = core
        .handle()
        .spawn_blocking(move || tailnet_for_start.start(&config, Arc::new(callback)))
        .await
        .map_err(CoreError::internal)?;
    if let Err(error) = result {
        return Err(map_error_redacted(error, secret.as_deref()));
    }

    let register_hook = {
        let mut state = state.0.lock().unwrap_or_else(|error| error.into_inner());
        state.started = true;
        if state.hook_registered {
            false
        } else {
            state.hook_registered = true;
            true
        }
    };
    if register_hook {
        let hook_state = state.clone();
        let hook_tailnet = tailnet;
        core.add_shutdown_hook(Box::new(move || {
            if let Err(error) = hook_tailnet.stop() {
                tracing::warn!(code = ?error.code, "tailnet stop during core shutdown failed");
            }
            let mut state = hook_state
                .0
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.started = false;
        }));
    }
    Ok(())
}

/// Stop the node.  Stopping an idle Core is intentionally idempotent.
#[cfg(unix)]
pub async fn stop(core: &Arc<Core>) -> Result<(), CoreError> {
    let state = core.state::<TailnetState>();
    let tailnet = {
        let state = state.0.lock().unwrap_or_else(|error| error.into_inner());
        if !state.started {
            return Ok(());
        }
        state
            .tailnet
            .clone()
            .ok_or_else(|| CoreError::internal("tailnet state is missing"))?
    };
    let result = blocking(core, move || tailnet.stop()).await;
    state
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .started = false;
    result.map(|_| ())
}

#[cfg(unix)]
async fn control<F>(core: &Arc<Core>, operation: F) -> Result<(), CoreError>
where
    F: FnOnce(Arc<Tailnet>) -> Result<(), TailnetError> + Send + 'static,
{
    let api = rs_tailnet::load_default()
        .map_err(|_| CoreError::unsupported("embedded tailnet is not available in this build"))?;
    let tailnet = {
        let state = core.state::<TailnetState>();
        let state = state.0.lock().unwrap_or_else(|error| error.into_inner());
        if !state.started {
            return Err(CoreError::internal("tailnet not started"));
        }
        state
            .tailnet
            .clone()
            .unwrap_or_else(|| Tailnet::global(api))
    };
    blocking(core, move || operation(tailnet)).await
}

#[cfg(unix)]
pub async fn login(core: &Arc<Core>) -> Result<(), CoreError> {
    control(core, |tailnet| tailnet.login()).await
}

#[cfg(unix)]
pub async fn logout(core: &Arc<Core>) -> Result<(), CoreError> {
    control(core, |tailnet| tailnet.logout()).await
}

#[cfg(unix)]
pub async fn rebind(core: &Arc<Core>) -> Result<(), CoreError> {
    control(core, |tailnet| tailnet.rebind()).await
}

/// Return the status JSON expected by the App boundary.
#[cfg(unix)]
pub async fn status_json(core: &Arc<Core>) -> Result<String, CoreError> {
    let api = match rs_tailnet::load_default() {
        Ok(api) => api,
        Err(_) => return Ok(
            r#"{"state":"unavailable","authUrl":null,"self":null,"magicDnsSuffix":"","peers":[]}"#
                .to_owned(),
        ),
    };
    let tailnet = Tailnet::global(api);
    let status = blocking(core, move || tailnet.status()).await?;
    let mut value = serde_json::to_value(status.clone())?;
    value["state"] = serde_json::Value::String(status.state.as_camel().to_owned());
    Ok(serde_json::to_string(&value)?)
}

/// Return a network dialer only while the most recently delivered state is
/// Running.  This function never calls into the blocking Go ABI.
#[cfg(unix)]
pub fn net_dialer(core: &Arc<Core>) -> Option<Arc<dyn NetDialer>> {
    let state = core.state::<TailnetState>();
    let state = state.0.lock().unwrap_or_else(|error| error.into_inner());
    if !state.started
        || state
            .last
            .as_ref()
            .is_none_or(|event| event.state != "running")
    {
        return None;
    }
    state.tailnet.as_ref().map(Tailnet::net_dialer)
}

#[cfg(not(unix))]
mod unsupported {
    use super::{Arc, Core, CoreError, NetDialer, TailnetStartOptions};

    pub fn available() -> bool {
        false
    }

    pub async fn start(_core: &Arc<Core>, _opts: TailnetStartOptions) -> Result<(), CoreError> {
        Err(super::unsupported_error())
    }

    pub async fn stop(_core: &Arc<Core>) -> Result<(), CoreError> {
        Err(super::unsupported_error())
    }

    pub async fn login(_core: &Arc<Core>) -> Result<(), CoreError> {
        Err(super::unsupported_error())
    }

    pub async fn logout(_core: &Arc<Core>) -> Result<(), CoreError> {
        Err(super::unsupported_error())
    }

    pub async fn rebind(_core: &Arc<Core>) -> Result<(), CoreError> {
        Err(super::unsupported_error())
    }

    pub async fn status_json(_core: &Arc<Core>) -> Result<String, CoreError> {
        Err(super::unsupported_error())
    }

    pub fn net_dialer(_core: &Arc<Core>) -> Option<Arc<dyn NetDialer>> {
        None
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn tailnet_available() -> bool {
    available()
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn tailnet_start(opts_json: String) -> Result<(), CoreError> {
    #[cfg(not(unix))]
    {
        let _ = opts_json;
        Err(unsupported_error())
    }
    #[cfg(unix)]
    {
        let opts = parse_start_options(&opts_json)?;
        let c = core()?;
        c.clone().run(async move { start(&c, opts).await }).await
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn tailnet_stop() -> Result<(), CoreError> {
    #[cfg(not(unix))]
    {
        Err(unsupported_error())
    }
    #[cfg(unix)]
    {
        let c = core()?;
        c.clone().run(async move { stop(&c).await }).await
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn tailnet_login() -> Result<(), CoreError> {
    #[cfg(not(unix))]
    {
        Err(unsupported_error())
    }
    #[cfg(unix)]
    {
        let c = core()?;
        c.clone().run(async move { login(&c).await }).await
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn tailnet_logout() -> Result<(), CoreError> {
    #[cfg(not(unix))]
    {
        Err(unsupported_error())
    }
    #[cfg(unix)]
    {
        let c = core()?;
        c.clone().run(async move { logout(&c).await }).await
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn tailnet_rebind() -> Result<(), CoreError> {
    #[cfg(not(unix))]
    {
        Err(unsupported_error())
    }
    #[cfg(unix)]
    {
        let c = core()?;
        c.clone().run(async move { rebind(&c).await }).await
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn tailnet_status() -> Result<String, CoreError> {
    #[cfg(not(unix))]
    {
        Err(unsupported_error())
    }
    #[cfg(unix)]
    {
        let c = core()?;
        c.clone().run(async move { status_json(&c).await }).await
    }
}
