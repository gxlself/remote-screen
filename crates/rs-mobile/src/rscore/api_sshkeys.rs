//! SSH user keys (M1 exposes only the device key).

use std::sync::Arc;

use rs_ssh::install::{install_keys, InstallError, InstallRequest, LineStatus, SkipReason};
use rs_ssh::{public_key_openssh, Credential, SshError};
use ssh_key::{HashAlg, PublicKey};
use tracing::warn;

use super::api_ssh::{connect_once, resolve_server, CorePrompter, PromptRelay};
use super::dto_core::{SshAuth, SshKey, SshKeyKind};
use super::error::CoreError;
use super::runtime::{core, Core};
use super::store::SshKeyRow;

pub const DEVICE_KEY_ID: &str = "device";
const DEVICE_KEY_KIND: &str = "se-p256";

/// Reconcile the persisted device-key metadata with the registered native
/// signer. The private key never leaves the secure element.
pub fn ensure_device_key(core: &Arc<Core>) -> Result<Option<SshKeyRow>, CoreError> {
    let Some(identity) = core.store().identity()? else {
        return Ok(None);
    };
    let Some(signer) = core.signer() else {
        return Ok(None);
    };
    let device_id = core.device_id()?;
    let public_key = public_key_openssh(
        &Credential::External(signer),
        &format!("rs-device={device_id}"),
    )
    .map_err(CoreError::internal)?;
    let parsed = PublicKey::from_openssh(&public_key).map_err(CoreError::internal)?;
    let fp = parsed.fingerprint(HashAlg::Sha256).to_string();

    let existing = core.store().ssh_key(DEVICE_KEY_ID)?;
    if existing
        .as_ref()
        .is_some_and(|row| row.public_key == public_key)
    {
        return Ok(existing);
    }

    let row = SshKeyRow {
        key_id: DEVICE_KEY_ID.to_owned(),
        name: identity.device_name,
        kind: DEVICE_KEY_KIND.to_owned(),
        fp,
        public_key,
        created_at_us: identity.created_at_us,
        installed_on: Vec::new(),
    };
    core.store().upsert_ssh_key(&row)?;
    Ok(Some(row))
}

pub fn keys(core: &Arc<Core>) -> Result<Vec<SshKey>, CoreError> {
    if ensure_device_key(core)?.is_none() {
        return Ok(Vec::new());
    }
    Ok(core
        .store()
        .ssh_keys()?
        .into_iter()
        .filter_map(ssh_key_from_row)
        .collect())
}

/// M1 "creation" names the existing secure-element key; it never generates or
/// persists private key material in Rust.
pub fn create_key(core: &Arc<Core>, kind: &str, name: &str) -> Result<SshKey, CoreError> {
    match kind {
        DEVICE_KEY_KIND => {
            let mut row = ensure_device_key(core)?
                .ok_or_else(|| CoreError::internal("identity not created"))?;
            let name = name.trim();
            if !name.is_empty() {
                row.name = name.to_owned();
                core.store().upsert_ssh_key(&row)?;
            }
            ssh_key_from_row(row).ok_or_else(|| CoreError::internal("invalid device key kind"))
        }
        "ed25519" => Err(CoreError::unsupported(
            "ed25519 keys need native secure storage",
        )),
        _ => Err(CoreError::internal("unknown key kind")),
    }
}

pub async fn install_key(core: &Arc<Core>, server_id: &str, key_id: &str) -> Result<(), CoreError> {
    if key_id != DEVICE_KEY_ID {
        return Err(CoreError::internal("unknown key"));
    }
    let resolved = resolve_server(core, server_id, None).await?;
    if resolved.auth == SshAuth::Tailscale {
        return Err(CoreError::unsupported(
            "tailscale ssh servers do not use device keys",
        ));
    }
    let row =
        ensure_device_key(core)?.ok_or_else(|| CoreError::internal("identity not created"))?;

    let prompter = Arc::new(CorePrompter::new(
        core.clone(),
        None,
        resolved.server_id.clone(),
        resolved.target.user.clone(),
        None,
        Arc::new(PromptRelay::default()),
    ));
    let session = connect_once(core, &resolved, prompter)
        .await
        .map_err(connect_error)?;
    let request = InstallRequest {
        device_id: core.device_id()?,
        device_key_line: row.public_key.clone(),
    };
    let result = install_keys(&session, &request).await;
    session.close().await;

    let report = result.map_err(install_error)?;
    match report.device {
        LineStatus::Added | LineStatus::AlreadyPresent => {}
        LineStatus::Skipped(SkipReason::TailscaleSsh) => {
            return Err(CoreError::unsupported(
                "tailscale ssh servers do not use device keys",
            ))
        }
    }

    if !core
        .store()
        .add_key_installed_on(DEVICE_KEY_ID, server_id)?
    {
        return Err(CoreError::internal("device key not found"));
    }

    Ok(())
}

fn ssh_key_from_row(row: SshKeyRow) -> Option<SshKey> {
    let kind = match row.kind.as_str() {
        DEVICE_KEY_KIND => SshKeyKind::SeP256,
        "ed25519" => SshKeyKind::Ed25519,
        other => {
            warn!(kind = other, "skipping unknown SSH key kind");
            return None;
        }
    };
    Some(SshKey {
        key_id: row.key_id,
        name: row.name,
        kind,
        fp: row.fp,
        public_key: row.public_key,
        created_at: row.created_at_us,
        installed_on: row.installed_on,
    })
}

pub(crate) fn connect_error(error: SshError) -> CoreError {
    match error {
        SshError::HostKeyRejected | SshError::HostKeyChanged { .. } => {
            CoreError::internal("host key not trusted; open a terminal to this server first")
        }
        SshError::AuthFailed { .. } | SshError::Cancelled => {
            CoreError::internal("authentication failed; open a terminal to this server first")
        }
        SshError::Timeout | SshError::Closed | SshError::Transport(_) => CoreError::network(error),
        SshError::Unsupported(_) => CoreError::unsupported(error),
        SshError::InvalidKey
        | SshError::SigningFailed
        | SshError::InvalidOptions(_)
        | SshError::RequestRejected => CoreError::internal(error),
    }
}

fn install_error(error: InstallError) -> CoreError {
    CoreError::internal(error)
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn ssh_keys() -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { Ok(serde_json::to_string(&keys(&c)?)?) })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn ssh_create_key(kind: String, name: String) -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { Ok(serde_json::to_string(&create_key(&c, &kind, &name)?)?) })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn ssh_install_key(server_id: String, key_id: String) -> Result<(), CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { install_key(&c, &server_id, &key_id).await })
        .await
}
