//! RsCore.adapters UniFFI exports.

use std::sync::{Arc, PoisonError};

use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

use super::driver::{Credentials, DriverInput};
use super::dto::{AdapterOpenOpts, AdapterTarget};
use super::session::{self, NetDial, SessionTimings};
use super::{secrets, table::AdapterEntry};
use crate::rscore::error::CoreError;
use crate::rscore::runtime::core;

// Bound credential inputs before passing them to native adapters.
const MAX_USER_BYTES: usize = 256;
const MAX_PASSWORD_BYTES: usize = 1024;
const MAX_DOMAIN_BYTES: usize = 256;

/// serde_json messages quote the offending value, which may be a password the
/// caller put in the wrong place; report only the kind and position.
fn parse_json<T: DeserializeOwned>(json: &str, what: &str) -> Result<T, CoreError> {
    serde_json::from_str(json).map_err(|error| {
        CoreError::internal(format!(
            "bad json: {what}: {:?} error at line {} column {}",
            error.classify(),
            error.line(),
            error.column()
        ))
    })
}

fn check_field(value: Option<&str>, max: usize, field: &str) -> Result<(), CoreError> {
    let value = value.unwrap_or_default();
    if value.len() > max || value.chars().any(char::is_control) {
        return Err(CoreError::internal(format!(
            "invalid credentials: {field} too long or has control characters"
        )));
    }
    Ok(())
}

/// The credentials submitted for this session while they are still held, else
/// the saved ones.
fn password_for(entry: &AdapterEntry) -> Option<Zeroizing<String>> {
    let pending = entry
        .pending_credentials
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(credentials, _)| Zeroizing::new(credentials.password.as_str().to_owned()));
    if pending.is_some() {
        return pending;
    }
    match secrets::load(&entry.store_key) {
        Ok(saved) => saved.map(|credentials| credentials.password),
        Err(error) => {
            tracing::warn!(
                session_id = entry.id,
                "failed to load saved adapter credentials: {error}"
            );
            None
        }
    }
}

/// target_json: AdapterTarget; opts_json: AdapterOpenOpts. Returns the adapter session id.
#[uniffi::export(async_runtime = "tokio")]
pub async fn adapters_open(target_json: String, opts_json: String) -> Result<u64, CoreError> {
    let c = core()?;
    let target: AdapterTarget = parse_json(&target_json, "adapter target")?;
    let opts: AdapterOpenOpts = parse_json(&opts_json, "adapter open options")?;
    c.clone()
        .run(async move {
            let driver = session::driver_for(target.protocol);
            let dial = Arc::new(NetDial::new(&c, &target, SessionTimings::default().connect));
            session::open_with(&c, target, opts, driver, dial).await
        })
        .await
}

#[uniffi::export]
pub fn adapters_close(id: u64) -> Result<(), CoreError> {
    let c = core()?;
    session::close(&c, id);
    Ok(())
}

/// JSON AdapterSummary[].
#[uniffi::export]
pub fn adapters_list() -> Result<String, CoreError> {
    let c = core()?;
    Ok(serde_json::to_string(&session::list(&c))?)
}

#[uniffi::export]
pub fn adapters_submit_credentials(
    id: u64,
    user: Option<String>,
    password: String,
    domain: Option<String>,
    save: bool,
) -> Result<(), CoreError> {
    let password = Zeroizing::new(password);
    let c = core()?;
    check_field(user.as_deref(), MAX_USER_BYTES, "user")?;
    check_field(Some(password.as_str()), MAX_PASSWORD_BYTES, "password")?;
    check_field(domain.as_deref(), MAX_DOMAIN_BYTES, "domain")?;
    session::submit_credentials(
        &c,
        id,
        Credentials {
            user,
            password,
            domain,
        },
        save,
    )
}

#[uniffi::export]
pub fn adapters_cancel_credentials(id: u64) -> Result<(), CoreError> {
    let c = core()?;
    session::cancel_credentials(&c, id);
    Ok(())
}

/// JSON SavedCredentialInfo for target_json (AdapterTarget).
#[uniffi::export]
pub fn adapters_saved(target_json: String) -> Result<String, CoreError> {
    let c = core()?;
    let target: AdapterTarget = parse_json(&target_json, "adapter target")?;
    Ok(serde_json::to_string(&secrets::saved_info(
        &c,
        &target.store_key(),
    ))?)
}

#[uniffi::export]
pub fn adapters_forget(target_json: String) -> Result<(), CoreError> {
    let c = core()?;
    let target: AdapterTarget = parse_json(&target_json, "adapter target")?;
    secrets::forget(&c, &target.store_key()).map_err(CoreError::internal)
}

/// Whether a native secret store is registered (C4.2).
#[uniffi::export]
pub fn adapters_can_save() -> bool {
    secrets::can_save()
}

#[uniffi::export]
pub fn adapters_type_password(id: u64) -> Result<(), CoreError> {
    let c = core()?;
    let entry =
        session::entry(&c, id).ok_or_else(|| CoreError::internal("unknown adapter session"))?;
    let password = password_for(&entry)
        .ok_or_else(|| CoreError::internal("no saved password for this adapter session"))?;
    session::send_input(&c, id, DriverInput::TypeSecret(password))
}

#[uniffi::export]
pub fn adapters_send_ctrl_alt_del(id: u64) -> Result<(), CoreError> {
    let c = core()?;
    session::send_input(&c, id, DriverInput::CtrlAltDel)
}

#[uniffi::export]
pub fn adapters_trust_cert(id: u64, accept: bool) -> Result<(), CoreError> {
    let c = core()?;
    session::trust_cert(&c, id, accept);
    Ok(())
}

/// View size changed (rotation, split view): RDP renegotiates its desktop size
/// (CONTRACT default 5); VNC ignores it.
#[uniffi::export]
pub fn adapters_resize(
    id: u64,
    view_width: u32,
    view_height: u32,
    scale: f32,
) -> Result<(), CoreError> {
    let c = core()?;
    if view_width == 0 || view_height == 0 || !scale.is_finite() || scale <= 0.0 {
        return Err(CoreError::internal("invalid view size"));
    }
    // Like adapters_close, a session that is gone is not an error.
    let _ = session::send_input(
        &c,
        id,
        DriverInput::Resize {
            view_width,
            view_height,
            scale,
        },
    );
    Ok(())
}

/// RDP: `count` monitors side by side (1..=4), changed at runtime over
/// displaycontrol or else by reconnecting with the session's credentials
/// (RS-MON1); VNC ignores it.
#[uniffi::export]
pub fn adapters_set_monitors(id: u64, count: u8) -> Result<(), CoreError> {
    let c = core()?;
    if !(1..=4).contains(&count) {
        return Err(CoreError::internal("invalid monitor count"));
    }
    // Like adapters_close, a session that is gone is not an error.
    let _ = session::send_input(&c, id, DriverInput::Monitors(count));
    Ok(())
}

/// Show one remote monitor (index into the last adapter.screens list) or, with
/// index 255, the whole desktop (RS-V4AE2).
#[uniffi::export]
pub fn adapters_select_screen(id: u64, index: u8) -> Result<(), CoreError> {
    let c = core()?;
    session::select_screen(&c, id, index)
}
