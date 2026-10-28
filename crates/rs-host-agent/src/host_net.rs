//! Platform-independent network information and listener hot reload support.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use host_core::config::{ConfigStore, HostConfig};
use host_core::ipc::NetworkFirewallStatusResult;
use host_core::listen::{ListenError, ListenPolicy, ListenerUpdater};
use host_core::platform::{Nic, Platform};
use serde_json::{json, Value};

use super::NetworkInfo;

/// Build the listener policy from the same fields used during startup and
/// configuration hot reload. `lan_cidrs` remains reserved for the later LAN
/// classification work.
pub fn listen_policy(config: &HostConfig, port: u16) -> ListenPolicy {
    ListenPolicy {
        tailnet: true,
        port,
        intranet_nics: if config.intranet.enabled {
            config.intranet.interfaces.clone()
        } else {
            Vec::new()
        },
        allowed_cidrs: config
            .intranet
            .allow_sources
            .iter()
            .filter_map(|source| source.parse().ok())
            .collect(),
        lan_cidrs: Vec::new(),
    }
}

/// Serialize a NIC in the shape consumed by the host UI network settings.
pub fn nic_json(nic: &Nic) -> Value {
    json!({
        "name": nic.name,
        "addresses": nic.addresses.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "networks": nic.networks,
        "mac": nic.mac.map(mac_string),
        "tailnet": nic.tailnet,
    })
}

fn mac_string(mac: [u8; 6]) -> String {
    let [a, b, c, d, e, f] = mac;
    format!("{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}")
}

/// Network information backed by any host-core platform implementation.
pub struct PlatformNetworkInfo<P> {
    platform: P,
    config: ConfigStore,
    port: u16,
}

impl<P> PlatformNetworkInfo<P> {
    pub fn new(platform: P, config: ConfigStore, port: u16) -> Self {
        Self {
            platform,
            config,
            port,
        }
    }
}

impl<P: Platform> NetworkInfo for PlatformNetworkInfo<P> {
    fn list_nics(&self) -> Vec<Value> {
        self.platform.nics().iter().map(nic_json).collect()
    }

    fn firewall_status(&self) -> NetworkFirewallStatusResult {
        NetworkFirewallStatusResult {
            enabled: false,
            active: false,
            detail: json!({"platform": "not_applicable"}),
        }
    }

    fn it_note(&self) -> String {
        let policy = listen_policy(&self.config.get(), self.port);
        let names = if policy.intranet_nics.is_empty() {
            "无".to_owned()
        } else {
            policy.intranet_nics.join("、")
        };
        format!(
            "Remote Host 使用 UDP 和 TCP {} 端口。\n选中的网卡：{}。",
            policy.port, names
        )
    }
}

/// A small abstraction around the live listener updater, so hot-reload tests
/// can record calls without opening sockets.
pub trait ListenerPolicyUpdater: Send + Sync {
    fn update(
        &self,
        policy: ListenPolicy,
        nics: Vec<Nic>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ListenError>> + Send + '_>>;
}

impl ListenerPolicyUpdater for ListenerUpdater {
    fn update(
        &self,
        policy: ListenPolicy,
        nics: Vec<Nic>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ListenError>> + Send + '_>> {
        Box::pin(ListenerUpdater::update(self, policy, nics))
    }
}

pub const DEFAULT_NIC_RESCAN_INTERVAL: Duration = Duration::from_secs(10);

/// Run the default-discovery listener hot-reload loop. Configuration changes
/// apply immediately; NIC snapshots are compared every ten seconds because
/// macOS has no address-change callback exposed through `Platform`.
pub async fn run_listener_updates<P: Platform>(
    config: ConfigStore,
    platform: P,
    port: u16,
    updater: Arc<dyn ListenerPolicyUpdater>,
) {
    let platform = Arc::new(platform);
    let mut config_changes = config.subscribe();
    let mut last_nics = snapshot_nics(&platform);
    let mut rescan = tokio::time::interval(DEFAULT_NIC_RESCAN_INTERVAL);
    // Avoid an immediate duplicate update for the snapshot already used by
    // `HostRuntime::bind`.
    rescan.tick().await;
    loop {
        tokio::select! {
            changed = config_changes.changed() => {
                if changed.is_err() {
                    return;
                }
                let nics = snapshot_nics(&platform);
                last_nics = nics.clone();
                let policy = listen_policy(config_changes.borrow().as_ref(), port);
                if matches!(updater.update(policy, nics).await, Err(ListenError::Closed)) {
                    return;
                }
            }
            _ = rescan.tick() => {
                let nics = snapshot_nics(&platform);
                if nics == last_nics {
                    continue;
                }
                last_nics = nics.clone();
                let policy = listen_policy(&config.get(), port);
                if matches!(updater.update(policy, nics).await, Err(ListenError::Closed)) {
                    return;
                }
            }
        }
    }
}

fn snapshot_nics<P: Platform>(platform: &Arc<P>) -> Vec<Nic> {
    platform.nics()
}

#[cfg(test)]
mod tests {
    use super::*;
    use host_core::platform::Nic;
    use host_fake::FakePlatform;
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let suffix = RandomState::new().build_hasher().finish();
            let path = std::env::temp_dir().join(format!(
                "rs-host-agent-{label}-{}-{:016x}",
                std::process::id(),
                suffix
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn nic(name: &str, address: &str) -> Nic {
        Nic {
            name: name.into(),
            addresses: vec![address.parse().unwrap()],
            networks: vec!["10.30.0.0/16".into()],
            mac: Some([0, 1, 2, 3, 4, 5]),
            tailnet: false,
        }
    }

    #[test]
    fn listen_policy_matches_startup_configuration_mapping() {
        let mut config = HostConfig::default();
        config.intranet.enabled = true;
        config.intranet.interfaces = vec!["en0".into()];
        config.intranet.allow_sources = vec!["10.30.0.0/16".into(), "bad".into()];
        let policy = listen_policy(&config, 7420);
        assert_eq!(policy.port, 7420);
        assert!(policy.tailnet);
        assert_eq!(policy.intranet_nics, vec!["en0"]);
        assert_eq!(policy.allowed_cidrs.len(), 1);
        assert!(policy.lan_cidrs.is_empty());

        config.intranet.enabled = false;
        assert!(listen_policy(&config, 7420).intranet_nics.is_empty());
    }

    #[test]
    fn nic_json_matches_network_ui_shape() {
        let value = nic_json(&Nic {
            name: "en0".into(),
            addresses: vec![IpAddr::V4(Ipv4Addr::new(10, 30, 12, 8))],
            networks: vec!["10.30.0.0/16".into()],
            mac: Some([0, 1, 2, 3, 4, 5]),
            tailnet: false,
        });
        assert_eq!(
            value,
            json!({
                "name": "en0",
                "addresses": ["10.30.12.8"],
                "networks": ["10.30.0.0/16"],
                "mac": "00:01:02:03:04:05",
                "tailnet": false,
            })
        );
    }

    #[test]
    fn platform_network_info_reports_platform_defaults() {
        let mut config = HostConfig::default();
        config.intranet.enabled = true;
        config.intranet.interfaces = vec!["en0".into()];
        let dir = TestDir::new("host-net-platform");
        let store = ConfigStore::new(dir.path().join("config.toml"), config);
        let info = PlatformNetworkInfo::new(
            FakePlatform::with_nics(vec![nic("en0", "10.30.12.8")]),
            store,
            7400,
        );
        assert_eq!(info.list_nics().len(), 1);
        assert_eq!(
            info.firewall_status().detail,
            json!({"platform": "not_applicable"})
        );
        assert_eq!(
            info.it_note(),
            "Remote Host 使用 UDP 和 TCP 7400 端口。\n选中的网卡：en0。"
        );
    }

    #[derive(Default)]
    struct RecordingUpdater {
        calls: Mutex<Vec<(ListenPolicy, Vec<Nic>)>>,
    }

    impl ListenerPolicyUpdater for RecordingUpdater {
        fn update(
            &self,
            policy: ListenPolicy,
            nics: Vec<Nic>,
        ) -> Pin<Box<dyn Future<Output = Result<(), ListenError>> + Send + '_>> {
            self.calls.lock().unwrap().push((policy, nics));
            Box::pin(async move { Ok(()) })
        }
    }

    struct ClosedUpdater;

    impl ListenerPolicyUpdater for ClosedUpdater {
        fn update(
            &self,
            _policy: ListenPolicy,
            _nics: Vec<Nic>,
        ) -> Pin<Box<dyn Future<Output = Result<(), ListenError>> + Send + '_>> {
            Box::pin(async { Err(ListenError::Closed) })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn config_and_nic_changes_trigger_updates() {
        async fn yield_workers() {
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        }

        let dir = TestDir::new("host-net-update");
        let config = ConfigStore::new(dir.path().join("config.toml"), HostConfig::default());
        let platform = FakePlatform::with_nics(vec![nic("en0", "10.30.12.8")]);
        let updater = Arc::new(RecordingUpdater::default());
        let task = tokio::spawn(run_listener_updates(
            config.clone(),
            platform.clone(),
            7400,
            updater.clone(),
        ));
        yield_workers().await;
        config
            .update(&json!({
                "intranet": {"enabled": true, "interfaces": ["en0"]}
            }))
            .unwrap();
        yield_workers().await;
        assert_eq!(updater.calls.lock().unwrap().len(), 1);
        assert_eq!(
            updater.calls.lock().unwrap()[0].0.intranet_nics,
            vec!["en0"]
        );
        task.abort();
        let _ = task.await;
    }

    #[tokio::test(start_paused = true)]
    async fn closed_listener_stops_hot_reload_task() {
        let dir = TestDir::new("host-net-closed");
        let config = ConfigStore::new(dir.path().join("config.toml"), HostConfig::default());
        let task = tokio::spawn(run_listener_updates(
            config.clone(),
            FakePlatform::with_nics(vec![nic("en0", "10.30.12.8")]),
            7400,
            Arc::new(ClosedUpdater),
        ));
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        config
            .update(&json!({"intranet": {"enabled": true}}))
            .unwrap();
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        assert!(task.is_finished());
        assert!(task.await.is_ok());
    }
}
