//! Device management operations exposed through the host-ui IPC backend.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use host_core::ipc::{
    DevicesRevokeParams, DevicesRevokeResult, DevicesUpdateParams, DevicesUpdateResult,
    EmptyResult, IpcError,
};
use host_core::platform::Platform;
use host_core::session::manager::{SessionControl, SessionManager};
use host_core::store::{PairingPath, PathPermission, PathPermissions, Store};

type DeviceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, IpcError>> + Send + 'a>>;

/// Operations needed by the local IPC backend for paired devices.
pub trait DevicesApi: Send + Sync + 'static {
    fn update(&self, params: DevicesUpdateParams) -> DeviceFuture<'_, DevicesUpdateResult>;
    fn revoke(&self, params: DevicesRevokeParams) -> DeviceFuture<'_, DevicesRevokeResult>;
}

/// Store and session-control implementation used by the real host agent.
pub struct DevicesApiImpl {
    store: Arc<Store>,
    control: Arc<dyn SessionControl>,
}

impl DevicesApiImpl {
    pub fn new<P: Platform>(manager: Arc<SessionManager<P>>, store: Arc<Store>) -> Self {
        Self {
            store,
            control: manager.session_control(),
        }
    }

    #[cfg(test)]
    fn with_control(store: Arc<Store>, control: Arc<dyn SessionControl>) -> Self {
        Self { store, control }
    }

    fn find_device(&self, device_id: &str) -> Result<host_core::store::PairedDevice, IpcError> {
        self.store
            .paired()
            .map_err(store_error)?
            .into_iter()
            .find(|device| device.device_id == device_id)
            .ok_or_else(|| IpcError::business("unknown_device", "device is not paired"))
    }
}

impl DevicesApi for DevicesApiImpl {
    fn update(&self, params: DevicesUpdateParams) -> DeviceFuture<'_, DevicesUpdateResult> {
        Box::pin(async move {
            let device = self.find_device(&params.device_id)?;
            let requested = requested_paths(&params.paths)?;
            let permission = match params.permission {
                host_core::ipc::Permission::ViewOnly => rsp_core::trust::Permission::ViewOnly,
                host_core::ipc::Permission::Control => rsp_core::trust::Permission::Control,
                host_core::ipc::Permission::Unattended => rsp_core::trust::Permission::Unattended,
            };
            self.store
                .set_permission(&device.fp, permission)
                .map_err(store_error)?;
            for (path, present) in requested {
                let existing = match path {
                    PairingPath::Tailnet => device.path_permissions.tailnet,
                    PairingPath::Intranet => device.path_permissions.intranet,
                    PairingPath::Lan => device.path_permissions.lan,
                };
                let next = if present {
                    if existing == PathPermission::ViewOnly {
                        PathPermission::ViewOnly
                    } else {
                        PathPermission::Allow
                    }
                } else {
                    PathPermission::Deny
                };
                self.store
                    .set_path_permission(&device.fp, path, next)
                    .map_err(store_error)?;
            }
            Ok(EmptyResult {})
        })
    }

    fn revoke(&self, params: DevicesRevokeParams) -> DeviceFuture<'_, DevicesRevokeResult> {
        Box::pin(async move {
            let device = self.find_device(&params.device_id)?;
            self.store.revoke(device.fp, 0).map_err(store_error)?;
            for session in self.control.sessions_list() {
                if session.device_id == params.device_id {
                    let _ = self.control.disconnect(&session.session_id);
                }
            }
            Ok(EmptyResult {})
        })
    }
}

fn requested_paths(paths: &[String]) -> Result<[(PairingPath, bool); 3], IpcError> {
    let mut requested = [false; 3];
    for path in paths {
        let index = match path.as_str() {
            "tailnet" => 0,
            "intranet" => 1,
            "lan" => 2,
            _ => return Err(IpcError::business("bad_path", "unknown device path")),
        };
        requested[index] = true;
    }
    Ok([
        (PairingPath::Tailnet, requested[0]),
        (PairingPath::Intranet, requested[1]),
        (PairingPath::Lan, requested[2]),
    ])
}

fn store_error(error: host_core::store::StoreError) -> IpcError {
    IpcError::business("store", error.to_string())
}

pub fn path_names(permissions: &PathPermissions) -> Vec<String> {
    [
        ("tailnet", permissions.tailnet),
        ("intranet", permissions.intranet),
        ("lan", permissions.lan),
    ]
    .into_iter()
    .filter(|(_, permission)| *permission != PathPermission::Deny)
    .map(|(path, _)| path.to_owned())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use host_core::ipc::{Permission, SessionRole, SessionSummary};
    use host_core::store::{DevicePlatform, PairedDevice};
    use std::sync::Mutex;

    struct FakeControl {
        sessions: Mutex<Vec<SessionSummary>>,
        disconnected: Mutex<Vec<String>>,
    }

    impl SessionControl for FakeControl {
        fn sessions_list(&self) -> Vec<SessionSummary> {
            self.sessions.lock().expect("sessions lock").clone()
        }

        fn disconnect(&self, session_id: &str) -> bool {
            let known = self
                .sessions
                .lock()
                .expect("sessions lock")
                .iter()
                .any(|session| session.session_id == session_id);
            if known {
                self.disconnected
                    .lock()
                    .expect("disconnect lock")
                    .push(session_id.to_owned());
            }
            known
        }

        fn set_view_only(&self, _session_id: &str) -> bool {
            false
        }

        fn approval_respond(
            &self,
            _request_id: &str,
            _allow: bool,
            _permission: Option<Permission>,
        ) -> bool {
            false
        }

        fn approval_pending(&self) -> Vec<serde_json::Value> {
            Vec::new()
        }
    }

    fn state_dir(name: &str) -> std::path::PathBuf {
        let root = std::env::var_os("TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("target"));
        root.join(format!(
            "rs-devices-ipc-{name}-{}-{}",
            std::process::id(),
            rsp_core::clock::unix_micros()
        ))
    }

    fn device(fp: [u8; 32], name: &str) -> PairedDevice {
        PairedDevice {
            fp,
            device_id: rsp_core::ids::id_from_fp(&fp),
            name: name.to_owned(),
            cert_der: Vec::new(),
            permission: rsp_core::trust::Permission::Control,
            paired_at_us: 1,
            require_confirm: false,
            platform: DevicePlatform::Ios,
            paired_via: PairingPath::Lan,
            whois_user: None,
            whois_node: None,
            whois_node_name: None,
            path_permissions: PathPermissions {
                tailnet: PathPermission::ViewOnly,
                intranet: PathPermission::Allow,
                lan: PathPermission::Allow,
            },
        }
    }

    fn api(store: Arc<Store>, control: Arc<FakeControl>) -> DevicesApiImpl {
        DevicesApiImpl::with_control(store, control)
    }

    #[tokio::test]
    async fn update_changes_permission_and_path_permissions() {
        let dir = state_dir("update");
        let store = Arc::new(Store::open(&dir).expect("store"));
        let fp = [7; 32];
        let paired = device(fp, "phone");
        let device_id = paired.device_id.clone();
        store.upsert(paired).expect("pair");
        let control = Arc::new(FakeControl {
            sessions: Mutex::new(Vec::new()),
            disconnected: Mutex::new(Vec::new()),
        });
        let api = api(store.clone(), control);
        api.update(DevicesUpdateParams {
            device_id,
            permission: Permission::Unattended,
            paths: vec!["tailnet".into()],
        })
        .await
        .expect("update");
        let updated = store.get(&fp).expect("get").expect("device");
        assert_eq!(updated.permission, rsp_core::trust::Permission::Unattended);
        assert_eq!(updated.path_permissions.tailnet, PathPermission::ViewOnly);
        assert_eq!(updated.path_permissions.intranet, PathPermission::Deny);
        assert_eq!(updated.path_permissions.lan, PathPermission::Deny);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn unknown_device_and_path_leave_store_unchanged() {
        let dir = state_dir("validation");
        let store = Arc::new(Store::open(&dir).expect("store"));
        let fp = [8; 32];
        let paired = device(fp, "phone");
        let device_id = paired.device_id.clone();
        store.upsert(paired).expect("pair");
        let control = Arc::new(FakeControl {
            sessions: Mutex::new(Vec::new()),
            disconnected: Mutex::new(Vec::new()),
        });
        let api = api(store.clone(), control);
        let unknown = api
            .update(DevicesUpdateParams {
                device_id: "unknown".into(),
                permission: Permission::Control,
                paths: vec![],
            })
            .await
            .expect_err("unknown device");
        assert_eq!(unknown.code, "unknown_device");
        let bad_path = api
            .update(DevicesUpdateParams {
                device_id,
                permission: Permission::ViewOnly,
                paths: vec!["other".into()],
            })
            .await
            .expect_err("bad path");
        assert_eq!(bad_path.code, "bad_path");
        let unchanged = store.get(&fp).expect("get").expect("device");
        assert_eq!(unchanged.permission, rsp_core::trust::Permission::Control);
        assert_eq!(
            unchanged.path_permissions,
            device(fp, "phone").path_permissions
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn revoke_disconnects_only_target_device_sessions() {
        let dir = state_dir("revoke");
        let store = Arc::new(Store::open(&dir).expect("store"));
        let target = device([9; 32], "target");
        let other = device([10; 32], "other");
        let target_id = target.device_id.clone();
        store.upsert(target).expect("target pair");
        store.upsert(other).expect("other pair");
        let control = Arc::new(FakeControl {
            sessions: Mutex::new(vec![
                SessionSummary {
                    session_id: "s-target-1".into(),
                    device_id: target_id.clone(),
                    role: SessionRole::Controller,
                    ..Default::default()
                },
                SessionSummary {
                    session_id: "s-target-2".into(),
                    device_id: target_id.clone(),
                    role: SessionRole::Viewer,
                    ..Default::default()
                },
                SessionSummary {
                    session_id: "s-other".into(),
                    device_id: rsp_core::ids::id_from_fp(&[10; 32]),
                    role: SessionRole::Viewer,
                    ..Default::default()
                },
            ]),
            disconnected: Mutex::new(Vec::new()),
        });
        let api = api(store.clone(), control.clone());
        api.revoke(DevicesRevokeParams {
            device_id: target_id,
        })
        .await
        .expect("revoke");
        assert_eq!(store.paired().expect("paired").len(), 1);
        assert!(store.is_revoked(&[9; 32]).expect("revoked"));
        let mut disconnected = control
            .disconnected
            .lock()
            .expect("disconnect lock")
            .clone();
        disconnected.sort();
        assert_eq!(disconnected, vec!["s-target-1", "s-target-2"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn list_paths_follow_non_denied_permissions() {
        let permissions = PathPermissions {
            tailnet: PathPermission::Allow,
            intranet: PathPermission::Deny,
            lan: PathPermission::ViewOnly,
        };
        assert_eq!(path_names(&permissions), vec!["tailnet", "lan"]);
    }
}
