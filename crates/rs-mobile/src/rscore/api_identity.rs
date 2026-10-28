//! Device identities backed exclusively by the registered native signer.

use std::sync::Arc;

use rsp_core::ids::{fp_b64url, id_from_fp};
use rsp_transport::identity::{spki_fingerprint, CertKind, LocalIdentity};

use super::dto_core::Identity;
use super::error::CoreError;
use super::runtime::{core, Core};
use super::signer::spki_matches;
use super::store::StoredIdentity;

/// Serialize identity creation so concurrent callers agree on one key.
#[derive(Default)]
pub struct IdentityOperations(pub tokio::sync::Mutex<()>);

pub async fn ensure(core: &Arc<Core>, device_name: &str) -> Result<Identity, CoreError> {
    let operations = core.state::<IdentityOperations>();
    let _operation = operations.0.lock().await;
    let signer = core
        .signer()
        .ok_or_else(|| CoreError::internal("device signer not registered"))?;
    let stored = core.store().identity()?;
    if let Some(mut stored) = stored {
        if spki_matches(signer.as_ref(), &stored.cert_der) {
            if stored.device_name != device_name {
                stored.device_name = device_name.to_owned();
                core.store().set_identity(&stored)?;
            }
            let fp = spki_fingerprint(&stored.cert_der).map_err(CoreError::internal)?;
            return Ok(Identity {
                device_id: id_from_fp(&fp),
                fp: fp_b64url(&fp),
            });
        }
    }

    let identity = tokio::task::spawn_blocking(move || {
        LocalIdentity::generate_external(CertKind::App, signer)
    })
    .await
    .map_err(CoreError::internal)?
    .map_err(CoreError::internal)?;
    let fp = spki_fingerprint(&identity.cert_der).map_err(CoreError::internal)?;
    core.store().set_identity(&StoredIdentity {
        cert_der: identity.cert_der.to_vec(),
        device_name: device_name.to_owned(),
        created_at_us: core.now_us(),
        registered_at_us: None,
    })?;
    core.identity_changed();
    Ok(Identity {
        device_id: id_from_fp(&fp),
        fp: fp_b64url(&fp),
    })
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn identity_ensure(device_name: String) -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { Ok(serde_json::to_string(&ensure(&c, &device_name).await?)?) })
        .await
}
