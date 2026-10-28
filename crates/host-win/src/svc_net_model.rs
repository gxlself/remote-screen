//! FriendlyName follow-up rules shared by listeners, firewall sync and the UI.
//!
//! Windows Wi-Fi random hardware addresses can change between networks. When
//! that happens, keep the missing configured name instead of guessing a NIC.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use host_core::config::HostConfig;
use host_core::listen::ListenPolicy;
use host_core::platform::Nic;
use serde_json::{json, Value};

pub const NIC_BOOK: &str = "nic_aliases.json";
pub const BIND_RETRY: Duration = Duration::from_secs(15);
pub type NicBook = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NicFollow {
    pub interfaces: Vec<String>,
    pub renames: Vec<(String, String)>,
    pub book: NicBook,
}

pub fn mac_string(mac: [u8; 6]) -> String {
    let [a, b, c, d, e, f] = mac;
    format!("{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}")
}

pub fn follow_nic_names(configured: &[String], nics: &[Nic], book: &NicBook) -> NicFollow {
    let configured_names: BTreeSet<_> = configured.iter().collect();
    let mut seen = BTreeSet::new();
    let mut claimed = BTreeSet::new();
    let mut result = NicFollow {
        book: book.clone(),
        ..Default::default()
    };
    for name in configured {
        if !seen.insert(name) {
            continue;
        }
        let mut final_name = name.clone();
        if let Some(nic) = nics.iter().find(|nic| nic.name == *name) {
            if let Some(mac) = nic.mac.filter(|mac| *mac != [0; 6]) {
                result.book.insert(name.clone(), mac_string(mac));
            }
        } else if let Some(mac) = book.get(name) {
            let mut candidates = nics.iter().filter(|nic| {
                !nic.tailnet
                    && !configured_names.contains(&nic.name)
                    && !claimed.contains(&nic.name)
                    && nic
                        .mac
                        .filter(|mac| *mac != [0; 6])
                        .is_some_and(|bytes| mac_string(bytes) == *mac)
            });
            if let Some(candidate) = candidates.next().filter(|_| candidates.next().is_none()) {
                final_name.clone_from(&candidate.name);
                result.renames.push((name.clone(), final_name.clone()));
                result.book.remove(name);
                result.book.insert(final_name.clone(), mac.clone());
                claimed.insert(final_name.clone());
            }
        }
        result.interfaces.push(final_name);
    }
    result
        .book
        .retain(|name, _| result.interfaces.contains(name));
    result
}

pub fn load_book(path: &Path) -> NicBook {
    crate::svc_os_model::load_json(path).unwrap_or_default()
}

pub fn save_book(path: &Path, book: &NicBook) -> std::io::Result<()> {
    crate::svc_os_model::save_json(path, book)
}

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

pub fn interfaces_patch(interfaces: &[String]) -> Value {
    json!({"intranet": {"interfaces": interfaces}})
}

pub fn nic_json(nic: &Nic) -> Value {
    json!({
        "name": nic.name,
        "addresses": nic.addresses.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "networks": nic.networks,
        "mac": nic.mac.map(mac_string),
        "tailnet": nic.tailnet,
    })
}
