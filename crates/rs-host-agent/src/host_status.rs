//! Status values shared by the local IPC backend and platform watchers.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use host_core::ipc::Event;
use host_core::platform::{Nic, Permissions, Platform};
use host_core::session::approval::PromptEvents;
use serde_json::{json, Value};
use tokio::task::JoinHandle;

/// Serialize the two platform permissions exposed by `status.get`.
pub fn permissions_json(permissions: Permissions) -> Value {
    json!({
        "screen_recording": permissions.screen_recording,
        "accessibility": permissions.accessibility,
    })
}

/// Return tailnet addresses with IPv4 addresses first and stable order within
/// each address family. An address reported by more than one interface appears
/// only once.
pub fn tailnet_addresses(nics: &[Nic]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ipv4 = Vec::new();
    let mut ipv6 = Vec::new();
    for address in nics
        .iter()
        .filter(|nic| nic.tailnet)
        .flat_map(|nic| nic.addresses.iter().copied())
    {
        if !seen.insert(address) {
            continue;
        }
        if address.is_ipv4() {
            ipv4.push(address.to_string());
        } else {
            ipv6.push(address.to_string());
        }
    }
    ipv4.extend(ipv6);
    ipv4
}

/// Source of the platform values needed by `status.get`.
pub trait StatusSource: Send + Sync + 'static {
    fn permissions(&self) -> Permissions;
    fn nics(&self) -> Vec<Nic>;
}

impl<P: Platform> StatusSource for P {
    fn permissions(&self) -> Permissions {
        Platform::permissions(self)
    }

    fn nics(&self) -> Vec<Nic> {
        Platform::nics(self)
    }
}

/// Polls a platform permission API and emits an event only when the value
/// changes. The default interval is two seconds because macOS TCC queries can
/// be slow and the setup UI polls at the same cadence.
pub struct PermissionWatcher {
    read: Arc<dyn Fn() -> Permissions + Send + Sync>,
    events: Arc<dyn PromptEvents>,
    interval: Duration,
}

impl PermissionWatcher {
    pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(2);

    pub fn new(
        read: Arc<dyn Fn() -> Permissions + Send + Sync>,
        events: Arc<dyn PromptEvents>,
        interval: Duration,
    ) -> Self {
        Self {
            read,
            events,
            interval: interval.max(Duration::from_millis(1)),
        }
    }

    pub fn with_default_interval(
        read: Arc<dyn Fn() -> Permissions + Send + Sync>,
        events: Arc<dyn PromptEvents>,
    ) -> Self {
        Self::new(read, events, Self::DEFAULT_INTERVAL)
    }

    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(self.run())
    }

    pub async fn run(self) {
        let mut interval = tokio::time::interval(self.interval);
        // Consume interval's immediate first tick, then establish a baseline.
        interval.tick().await;
        let mut previous = match Self::read(Arc::clone(&self.read)).await {
            Some(value) => value,
            None => return,
        };
        loop {
            interval.tick().await;
            let Some(current) = Self::read(Arc::clone(&self.read)).await else {
                return;
            };
            if current == previous {
                continue;
            }
            previous = current;
            let _ = self
                .events
                .emit(Event::PermissionsChanged(permissions_json(current)));
        }
    }

    async fn read(read: Arc<dyn Fn() -> Permissions + Send + Sync>) -> Option<Permissions> {
        tokio::task::spawn_blocking(move || read()).await.ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use host_core::ipc::Event;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::sync::Mutex;

    fn nic(name: &str, addresses: &[IpAddr], tailnet: bool) -> Nic {
        Nic {
            name: name.into(),
            addresses: addresses.to_vec(),
            networks: Vec::new(),
            mac: None,
            tailnet,
        }
    }

    #[test]
    fn permissions_json_has_exactly_the_public_keys() {
        let value = permissions_json(Permissions {
            screen_recording: true,
            accessibility: false,
        });
        assert_eq!(
            value,
            json!({
                "screen_recording": true,
                "accessibility": false,
            })
        );
        assert_eq!(value.as_object().map(|object| object.len()), Some(2));
    }

    #[test]
    fn tailnet_addresses_filters_deduplicates_and_places_ipv4_first() {
        let addresses = tailnet_addresses(&[
            nic(
                "utun0",
                &[
                    IpAddr::V6(Ipv6Addr::LOCALHOST),
                    IpAddr::V4(Ipv4Addr::new(100, 64, 0, 7)),
                ],
                true,
            ),
            nic("en0", &[IpAddr::V4(Ipv4Addr::new(192, 168, 31, 20))], false),
            nic(
                "utun1",
                &[
                    IpAddr::V4(Ipv4Addr::new(100, 64, 0, 7)),
                    IpAddr::V6("fd7a:115c:a1e0::7".parse().unwrap()),
                ],
                true,
            ),
        ]);
        assert_eq!(addresses, vec!["100.64.0.7", "::1", "fd7a:115c:a1e0::7",]);
    }

    #[derive(Default)]
    struct Events {
        values: Mutex<Vec<Event>>,
        notified: tokio::sync::Notify,
    }

    impl PromptEvents for Events {
        fn emit(&self, event: Event) -> bool {
            self.values.lock().unwrap().push(event);
            self.notified.notify_one();
            true
        }
    }

    #[tokio::test(start_paused = true)]
    async fn permission_watcher_emits_only_permission_changes() {
        async fn yield_workers() {
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
        }

        let values = Arc::new(Events::default());
        let sequence = Arc::new(Mutex::new(vec![
            Permissions {
                screen_recording: false,
                accessibility: false,
            },
            Permissions {
                screen_recording: true,
                accessibility: false,
            },
            Permissions {
                screen_recording: true,
                accessibility: false,
            },
            Permissions {
                screen_recording: true,
                accessibility: true,
            },
        ]));
        let read_sequence = Arc::clone(&sequence);
        let read_notified = Arc::new(tokio::sync::Notify::new());
        let read_notified_in_reader = Arc::clone(&read_notified);
        let watcher = PermissionWatcher::new(
            Arc::new(move || {
                let value = read_sequence.lock().unwrap().remove(0);
                read_notified_in_reader.notify_one();
                value
            }),
            Arc::clone(&values) as Arc<dyn PromptEvents>,
            Duration::from_secs(2),
        );
        let baseline_read = read_notified.notified();
        let task = watcher.spawn();
        baseline_read.await;
        yield_workers().await;
        assert!(values.values.lock().unwrap().is_empty());

        let first_event = values.notified.notified();
        tokio::time::advance(Duration::from_secs(2)).await;
        first_event.await;
        yield_workers().await;
        assert_eq!(values.values.lock().unwrap().len(), 1);
        assert_eq!(
            values.values.lock().unwrap()[0],
            Event::PermissionsChanged(json!({
                "screen_recording": true,
                "accessibility": false,
            }))
        );

        let unchanged_read = read_notified.notified();
        tokio::time::advance(Duration::from_secs(2)).await;
        unchanged_read.await;
        yield_workers().await;
        assert_eq!(values.values.lock().unwrap().len(), 1);
        let second_event = values.notified.notified();
        tokio::time::advance(Duration::from_secs(2)).await;
        second_event.await;
        yield_workers().await;
        assert_eq!(values.values.lock().unwrap().len(), 2);
        assert_eq!(
            values.values.lock().unwrap()[1],
            Event::PermissionsChanged(json!({
                "screen_recording": true,
                "accessibility": true,
            }))
        );
        task.abort();
        let _ = task.await;
    }
}
