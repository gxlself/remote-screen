//! Resolve the local Tailscale login as the Host owner.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::{
    platform::Platform,
    session::manager::SessionManager,
    update::types::{Arch, HostOs},
    whois::{ChainedWhoIs, WhoIsError},
};

#[async_trait]
pub trait OwnerSource: Send + Sync {
    async fn self_login(&self) -> Result<String, WhoIsError>;
}

#[async_trait]
impl OwnerSource for ChainedWhoIs {
    async fn self_login(&self) -> Result<String, WhoIsError> {
        ChainedWhoIs::self_login(self).await
    }
}

pub struct OwnerOptions {
    pub owner: Arc<dyn OwnerSource>,
    pub owner_retry: Duration,
}

impl OwnerOptions {
    pub fn new(owner: Arc<dyn OwnerSource>) -> Self {
        Self {
            owner,
            owner_retry: Duration::from_secs(60),
        }
    }
}

/// Owner retries do not keep the manager alive; the task ends once the owner
/// is resolved or the manager is dropped.
pub fn spawn<P: Platform>(mgr: Arc<SessionManager<P>>, opts: OwnerOptions) -> JoinHandle<()> {
    let weak = Arc::downgrade(&mgr);
    let clock = mgr.services().clock.clone();
    tokio::spawn(async move {
        let gone = async {
            while weak.strong_count() != 0 {
                clock.sleep(Duration::from_secs(1)).await;
            }
        };
        tokio::select! {
            _ = gone => {},
            _ = async {
                let mut failures = 0u64;
                loop {
                    let result = opts.owner.self_login().await.and_then(|login| {
                        if login.trim().is_empty() {
                            Err(WhoIsError::Json("empty Self login".into()))
                        } else {
                            Ok(login)
                        }
                    });
                    match result {
                        Ok(login) => {
                            if let Some(mgr) = weak.upgrade() {
                                mgr.set_owner_login(Some(login));
                            }
                            return;
                        }
                        Err(error) => {
                            failures = failures.saturating_add(1);
                            let retry_s = opts.owner_retry.as_secs_f64();
                            if failures == 1 || failures.is_multiple_of(10) {
                                warn!(%error, retry_s, "tailscale login unavailable; owner not resolved");
                            } else {
                                debug!(%error, retry_s, "tailscale login unavailable; owner not resolved");
                            }
                            clock.sleep(opts.owner_retry).await;
                        }
                    }
                }
            } => {},
        }
    })
}

pub fn host_os() -> HostOs {
    if cfg!(target_os = "macos") {
        HostOs::Macos
    } else if cfg!(target_os = "windows") {
        HostOs::Windows
    } else if cfg!(target_os = "linux") {
        HostOs::Linux
    } else {
        HostOs::Unknown
    }
}

pub fn host_arch() -> Arch {
    if cfg!(target_os = "macos") {
        Arch::Universal
    } else {
        match std::env::consts::ARCH {
            "x86_64" => Arch::X64,
            "aarch64" => Arch::Arm64,
            _ => Arch::Unknown,
        }
    }
}
