//! 10-24 Tailscale API token exports (`RsCoreApi.tailscaleApi`). They need
//! only the registered SecretStore, not an initialized Core, and never return
//! the token.

use std::sync::{Arc, OnceLock};

use zeroize::Zeroizing;

use super::adapters::secrets::{registered, SecretStore};
use super::error::CoreError;
use super::tsapi::TsApi;

fn api() -> &'static TsApi {
    static API: OnceLock<TsApi> = OnceLock::new();
    API.get_or_init(TsApi::production)
}

fn store() -> Result<Arc<dyn SecretStore>, CoreError> {
    registered().ok_or_else(|| CoreError::unsupported("secret store unavailable"))
}

/// List devices with `token` and save it only if that succeeds. A non-empty
/// `control_url` is a Headscale server (`https://host[:port]`, else
/// `Internal("bad control url")` before any request); otherwise the official
/// Tailscale API. Returns JSON `TailscaleDevice[]`.
#[uniffi::export(async_runtime = "tokio")]
pub async fn tailscale_api_set_token(
    token: String,
    control_url: Option<String>,
) -> Result<String, CoreError> {
    let token = Zeroizing::new(token);
    let store = store()?;
    let devices = api()
        .set_token(store.as_ref(), &token, control_url.as_deref())
        .await?;
    Ok(serde_json::to_string(&devices)?)
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn tailscale_api_clear_token() -> Result<(), CoreError> {
    let store = store()?;
    api().clear_token(store.as_ref()).await
}

/// JSON `TailscaleApiStatus`.
#[uniffi::export(async_runtime = "tokio")]
pub async fn tailscale_api_status() -> Result<String, CoreError> {
    let store = store()?;
    Ok(serde_json::to_string(&api().status(store.as_ref())?)?)
}

/// JSON `TailscaleDevice[]`; cached for 30 s unless `force`.
#[uniffi::export(async_runtime = "tokio")]
pub async fn tailscale_api_devices(force: bool) -> Result<String, CoreError> {
    let store = store()?;
    let devices = api().devices(store.as_ref(), force).await?;
    Ok(serde_json::to_string(&devices)?)
}
