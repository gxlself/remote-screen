//! Platform-independent Windows firewall rule model.
//!
//! The COM adapter consumes the specifications in this module.  Keeping the
//! calculations here free of Windows APIs makes them deterministic and
//! testable on every target.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};

use host_core::ipc::NetworkFirewallStatusResult;
use host_core::listen::{bind_addrs, ListenPolicy, PathClass};
use host_core::platform::Nic;
use ipnet::IpNet;
use serde_json::{json, Value};

pub const RULE_GROUP: &str = "Remote Host";
pub const TAILNET_PREFIX: &str = "Remote Host (tailnet)";
pub const INTRANET_PREFIX: &str = "Remote Host (内网)";
pub const TAILNET_RULE_UDP: &str = "Remote Host (tailnet) UDP";
pub const TAILNET_RULE_TCP: &str = "Remote Host (tailnet) TCP";
pub const INTRANET_RULE_UDP: &str = "Remote Host (内网) UDP";
pub const INTRANET_RULE_TCP: &str = "Remote Host (内网) TCP";
pub const TAILNET_V4: &str = "100.64.0.0/10";
pub const TAILNET_V6: &str = "fd7a:115c:a1e0::/48";
pub const PROTO_TCP: i32 = 6;
pub const PROTO_UDP: i32 = 17;
pub const PROFILE_DOMAIN: i32 = 1;
pub const PROFILE_PRIVATE: i32 = 2;
pub const PROFILE_PUBLIC: i32 = 4;
pub const PROFILE_ALL: i32 = 0x7FFF_FFFF;

/// One inbound allow rule.  Direction and action are fixed by the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FwRuleSpec {
    pub name: String,
    pub description: String,
    pub group: String,
    pub protocol: i32,
    pub local_ports: String,
    pub local_addresses: String,
    pub remote_addresses: String,
    pub profiles: i32,
    pub application: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntranetSkip {
    Disabled,
    NoSources,
    NoLocalAddresses,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntranetPlan {
    pub rules: Vec<FwRuleSpec>,
    pub skipped: Option<IntranetSkip>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FwPlan {
    pub remove_names: Vec<String>,
    pub add: Vec<FwRuleSpec>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProfileState {
    pub domain: bool,
    pub private: bool,
    pub public: bool,
    pub current: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FwFacts {
    pub profiles: ProfileState,
    pub policy_locked: bool,
    pub existing: Vec<FwRuleSpec>,
    pub error: Option<String>,
}

/// Return the two rules that cover the fixed tailnet ranges.
pub fn tailnet_rules(port: u16, exe: Option<&str>) -> Vec<FwRuleSpec> {
    let addresses = format!("{TAILNET_V4},{TAILNET_V6}");
    [(TAILNET_RULE_UDP, PROTO_UDP), (TAILNET_RULE_TCP, PROTO_TCP)]
        .into_iter()
        .map(|(name, protocol)| FwRuleSpec {
            name: name.to_owned(),
            description: "Remote Host 经 tailnet 的入站连接".to_owned(),
            group: RULE_GROUP.to_owned(),
            protocol,
            local_ports: port.to_string(),
            local_addresses: addresses.clone(),
            remote_addresses: addresses.clone(),
            profiles: PROFILE_ALL,
            application: exe.map(str::to_owned),
            enabled: true,
        })
        .collect()
}

/// Build rules for selected non-tailnet addresses and source CIDRs.
pub fn intranet_rules(policy: &ListenPolicy, nics: &[Nic], exe: Option<&str>) -> IntranetPlan {
    if policy.intranet_nics.is_empty() {
        return IntranetPlan {
            rules: Vec::new(),
            skipped: Some(IntranetSkip::Disabled),
        };
    }
    if policy.allowed_cidrs.is_empty() {
        return IntranetPlan {
            rules: Vec::new(),
            skipped: Some(IntranetSkip::NoSources),
        };
    }

    let local_addresses = bind_addrs(policy, nics)
        .into_iter()
        .filter_map(|(address, path)| match path {
            PathClass::Intranet | PathClass::Lan => Some(address.ip()),
            PathClass::Tailnet => None,
        })
        .collect::<Vec<_>>();
    let local_addresses = sorted_ips(local_addresses);
    if local_addresses.is_empty() {
        return IntranetPlan {
            rules: Vec::new(),
            skipped: Some(IntranetSkip::NoLocalAddresses),
        };
    }

    let remote_addresses = sorted_nets(policy.allowed_cidrs.iter().map(IpNet::trunc));
    let local = local_addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let remote = remote_addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let rules = [
        (INTRANET_RULE_UDP, PROTO_UDP),
        (INTRANET_RULE_TCP, PROTO_TCP),
    ]
    .into_iter()
    .map(|(name, protocol)| FwRuleSpec {
        name: name.to_owned(),
        description: "Remote Host 内网 / 局域网入站连接".to_owned(),
        group: RULE_GROUP.to_owned(),
        protocol,
        local_ports: policy.port.to_string(),
        local_addresses: local.clone(),
        remote_addresses: remote.clone(),
        profiles: PROFILE_DOMAIN | PROFILE_PRIVATE,
        application: exe.map(str::to_owned),
        enabled: true,
    })
    .collect();
    IntranetPlan {
        rules,
        skipped: None,
    }
}

/// Normalize a comma-separated Windows address list.
pub fn normalize_addresses(s: &str) -> Vec<String> {
    let tokens = s
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens
        .iter()
        .any(|token| token.eq_ignore_ascii_case("*") || token.eq_ignore_ascii_case("any"))
    {
        return vec!["*".to_owned()];
    }

    let mut result = BTreeSet::new();
    for token in tokens {
        result.insert(normalize_address_token(token));
    }
    result.into_iter().collect()
}

fn normalize_address_token(token: &str) -> String {
    if let Some((address, mask)) = token.split_once('/') {
        if let (Ok(ip), Ok(netmask)) = (
            address.trim().parse::<Ipv4Addr>(),
            mask.trim().parse::<Ipv4Addr>(),
        ) {
            if let Some(prefix) = ipv4_netmask_prefix(netmask) {
                if let Ok(network) = IpNet::new(IpAddr::V4(ip), prefix) {
                    return network.trunc().to_string();
                }
            }
            return token.to_ascii_lowercase();
        }

        if let Ok(network) = token.parse::<IpNet>() {
            return network.trunc().to_string().to_ascii_lowercase();
        }
        return token.to_ascii_lowercase();
    }

    if let Ok(ip) = token.parse::<IpAddr>() {
        let prefix = match ip {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if let Ok(network) = IpNet::new(ip, prefix) {
            return network.to_string().to_ascii_lowercase();
        }
    }
    token.to_ascii_lowercase()
}

fn ipv4_netmask_prefix(mask: Ipv4Addr) -> Option<u8> {
    let value = u32::from(mask);
    (0..=32).find(|prefix| {
        let expected = if *prefix == 0 {
            0
        } else {
            u32::MAX << (32 - *prefix)
        };
        value == expected
    })
}

/// Normalize a comma-separated port list. Numeric values sort before textual
/// values, and textual values sort lexicographically.
pub fn normalize_ports(s: &str) -> Vec<String> {
    let tokens = s
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if tokens
        .iter()
        .any(|token| token.eq_ignore_ascii_case("*") || token.eq_ignore_ascii_case("any"))
    {
        return vec!["*".to_owned()];
    }

    let mut values = tokens
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    values.sort_by(port_cmp);
    values.dedup();
    values
}

fn port_cmp(left: &String, right: &String) -> Ordering {
    match (left.parse::<u32>(), right.parse::<u32>()) {
        (Ok(left_number), Ok(right_number)) => {
            left_number.cmp(&right_number).then_with(|| left.cmp(right))
        }
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => left.cmp(right),
    }
}

pub fn normalize_profiles(p: i32) -> i32 {
    if p == PROFILE_ALL || p & 7 == 7 {
        7
    } else {
        p & 7
    }
}

pub fn rule_matches(desired: &FwRuleSpec, actual: &FwRuleSpec) -> bool {
    desired.name == actual.name
        && desired.protocol == actual.protocol
        && normalize_ports(&desired.local_ports) == normalize_ports(&actual.local_ports)
        && normalize_addresses(&desired.local_addresses)
            == normalize_addresses(&actual.local_addresses)
        && normalize_addresses(&desired.remote_addresses)
            == normalize_addresses(&actual.remote_addresses)
        && normalize_profiles(desired.profiles) == normalize_profiles(actual.profiles)
        && desired.enabled == actual.enabled
        && desired.group == actual.group
        && match (&desired.application, &actual.application) {
            (None, None) => true,
            (Some(left), Some(right)) => left.to_lowercase() == right.to_lowercase(),
            _ => false,
        }
}

pub fn tailnet_ok(existing: &[FwRuleSpec], port: u16) -> bool {
    let port = port.to_string();
    [(TAILNET_RULE_UDP, PROTO_UDP), (TAILNET_RULE_TCP, PROTO_TCP)]
        .into_iter()
        .all(|(name, protocol)| {
            existing.iter().any(|rule| {
                rule.name == name
                    && rule.enabled
                    && rule.protocol == protocol
                    && normalize_ports(&rule.local_ports).contains(&port)
                    && normalize_addresses(&rule.remote_addresses).contains(&TAILNET_V4.to_owned())
            })
        })
}

pub fn plan(managed_prefix: &str, desired: &[FwRuleSpec], existing: &[FwRuleSpec]) -> FwPlan {
    let desired_by_name = desired
        .iter()
        .filter(|rule| rule.name.starts_with(managed_prefix))
        .map(|rule| (rule.name.as_str(), rule))
        .collect::<BTreeMap<_, _>>();
    let mut existing_by_name = BTreeMap::<String, Vec<&FwRuleSpec>>::new();
    for rule in existing
        .iter()
        .filter(|rule| rule.name.starts_with(managed_prefix))
    {
        existing_by_name
            .entry(rule.name.clone())
            .or_default()
            .push(rule);
    }

    let mut remove_names = Vec::new();
    for (name, rules) in &existing_by_name {
        let matches = desired_by_name
            .get(name.as_str())
            .is_some_and(|desired_rule| rules.len() == 1 && rule_matches(desired_rule, rules[0]));
        if !matches {
            remove_names.push(name.clone());
        }
    }

    let mut add = Vec::new();
    for desired_rule in desired
        .iter()
        .filter(|rule| rule.name.starts_with(managed_prefix))
    {
        let should_add = match existing_by_name.get(&desired_rule.name) {
            Some(rules) => rules.len() != 1 || !rule_matches(desired_rule, rules[0]),
            None => true,
        };
        if should_add {
            add.push(desired_rule.clone());
        }
    }

    FwPlan { remove_names, add }
}

pub fn plan_intranet(desired: &IntranetPlan, existing: &[FwRuleSpec]) -> FwPlan {
    plan(INTRANET_PREFIX, &desired.rules, existing)
}

pub fn status(facts: &FwFacts, desired: &IntranetPlan, port: u16) -> NetworkFirewallStatusResult {
    let enabled = facts.profiles.domain || facts.profiles.private;
    let active = desired.rules.is_empty()
        || desired.rules.iter().all(|desired_rule| {
            let matching = facts
                .existing
                .iter()
                .filter(|actual| actual.name == desired_rule.name)
                .collect::<Vec<_>>();
            matching.len() == 1 && rule_matches(desired_rule, matching[0])
        });

    let current_profiles = [
        (PROFILE_DOMAIN, "domain"),
        (PROFILE_PRIVATE, "private"),
        (PROFILE_PUBLIC, "public"),
    ]
    .into_iter()
    .filter(|(bit, _)| facts.profiles.current & bit != 0)
    .map(|(_, name)| Value::String(name.to_owned()))
    .collect::<Vec<_>>();

    let intranet = desired
        .rules
        .iter()
        .map(|desired_rule| {
            let matching = facts
                .existing
                .iter()
                .filter(|actual| actual.name == desired_rule.name)
                .collect::<Vec<_>>();
            let state = match matching.len() {
                0 => "missing",
                1 if rule_matches(desired_rule, matching[0]) => "ok",
                1 => "mismatch",
                _ => "duplicate",
            };
            json!({"name": desired_rule.name, "state": state})
        })
        .collect::<Vec<_>>();

    let rules = desired
        .rules
        .iter()
        .map(|rule| {
            json!({
                "name": rule.name,
                "protocol": protocol_name(rule.protocol),
                "local_port": rule.local_ports,
                "local_addresses": rule.local_addresses,
                "remote_addresses": rule.remote_addresses,
                "profiles": profile_name(rule.profiles),
                "program": rule.application,
            })
        })
        .collect::<Vec<_>>();

    let skipped = desired.skipped.map(|value| match value {
        IntranetSkip::Disabled => "disabled",
        IntranetSkip::NoSources => "no_sources",
        IntranetSkip::NoLocalAddresses => "no_local_addresses",
    });
    let detail = json!({
        "profiles": {
            "domain": facts.profiles.domain,
            "private": facts.profiles.private,
            "public": facts.profiles.public,
        },
        "current_profiles": current_profiles,
        "policy_locked": facts.policy_locked,
        "tailnet": if tailnet_ok(&facts.existing, port) { "ok" } else { "missing" },
        "intranet": intranet,
        "skipped": skipped,
        "error": facts.error,
        "rules": rules,
    });
    NetworkFirewallStatusResult {
        enabled,
        active,
        detail,
    }
}

pub fn netsh_add_command(spec: &FwRuleSpec) -> String {
    let local = netsh_addresses(&spec.local_addresses);
    let remote = netsh_addresses(&spec.remote_addresses);
    let ports = if normalize_ports(&spec.local_ports) == ["*"] {
        "any"
    } else {
        &spec.local_ports
    };
    let profile = profile_name(spec.profiles);
    let mut command = format!(
        "netsh advfirewall firewall add rule name=\"{}\" dir=in action=allow protocol={} localport={} localip={} remoteip={} profile={}",
        spec.name,
        protocol_name(spec.protocol),
        ports,
        local,
        remote,
        profile,
    );
    if let Some(application) = &spec.application {
        command.push_str(&format!(" program=\"{application}\""));
    }
    command.push_str(if spec.enabled {
        " enable=yes"
    } else {
        " enable=no"
    });
    command
}

fn netsh_addresses(addresses: &str) -> String {
    if normalize_addresses(addresses) == ["*"] {
        "any".to_owned()
    } else {
        addresses.to_owned()
    }
}

fn protocol_name(protocol: i32) -> &'static str {
    match protocol {
        PROTO_UDP => "UDP",
        PROTO_TCP => "TCP",
        _ => "UNKNOWN",
    }
}

fn profile_name(profiles: i32) -> String {
    match normalize_profiles(profiles) {
        7 => "any".to_owned(),
        value => [
            (PROFILE_DOMAIN, "domain"),
            (PROFILE_PRIVATE, "private"),
            (PROFILE_PUBLIC, "public"),
        ]
        .into_iter()
        .filter(|(bit, _)| value & bit != 0)
        .map(|(_, name)| name)
        .collect::<Vec<_>>()
        .join(","),
    }
}

pub fn it_note(
    policy: &ListenPolicy,
    nics: &[Nic],
    exe: Option<&str>,
    policy_locked: bool,
) -> String {
    let mut lines = vec![format!(
        "Remote Host 使用 UDP 和 TCP {} 端口。",
        policy.port
    )];
    let sources = policy
        .allowed_cidrs
        .iter()
        .map(IpNet::trunc)
        .map(|network| network.to_string())
        .collect::<Vec<_>>();
    if sources.is_empty() {
        lines.push("还没有设置来源网段。".to_owned());
    } else {
        lines.push(format!(
            "请允许来源网段 {} 访问本机。",
            join_sources(&sources)
        ));
    }

    let tailnet_policy = ListenPolicy {
        tailnet: true,
        ..policy.clone()
    };
    let tailnet_addresses = bind_addrs(&tailnet_policy, nics)
        .into_iter()
        .filter_map(|(address, path)| (path == PathClass::Tailnet).then_some(address.ip()))
        .collect::<Vec<_>>();
    let local_bindings = local_bindings(policy, nics);
    let tailnet_text = join_ips(tailnet_addresses);
    let local_text = if local_bindings.is_empty() {
        "无".to_owned()
    } else {
        local_bindings
            .iter()
            .map(|(ip, name)| format!("{ip}（{name}）"))
            .collect::<Vec<_>>()
            .join("、")
    };
    lines.push(format!(
        "tailnet 地址：{}；局域网地址：{}。",
        if tailnet_text.is_empty() {
            "无"
        } else {
            &tailnet_text
        },
        local_text
    ));
    if policy_locked {
        lines.push(
            "这台电脑的防火墙由组策略管理，本机程序加不了规则，请在组策略里添加下面的入站规则（仅「域」「专用」网络）："
                .to_owned(),
        );
    } else {
        lines.push("如需手动添加，入站规则如下（仅「域」「专用」网络）：".to_owned());
    }

    let intranet = intranet_rules(policy, nics, exe);
    if intranet.rules.is_empty() {
        lines.push("（内网连接未打开或没有可用的网卡地址，暂不需要规则）".to_owned());
    } else {
        lines.extend(intranet.rules.iter().map(netsh_add_command));
    }
    lines.join("\n")
}

fn join_sources(sources: &[String]) -> String {
    match sources.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, prefix)) => format!("{} 与 {last}", prefix.join("、")),
    }
}

fn join_ips(ips: Vec<IpAddr>) -> String {
    ips.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("、")
}

fn sorted_ips<I>(ips: I) -> Vec<IpAddr>
where
    I: IntoIterator<Item = IpAddr>,
{
    let mut result = ips.into_iter().collect::<Vec<_>>();
    result.sort_by(ip_cmp);
    result.dedup();
    result
}

fn ip_cmp(left: &IpAddr, right: &IpAddr) -> Ordering {
    match (left, right) {
        (IpAddr::V4(left), IpAddr::V4(right)) => left.cmp(right),
        (IpAddr::V6(left), IpAddr::V6(right)) => left.cmp(right),
        (IpAddr::V4(_), IpAddr::V6(_)) => Ordering::Less,
        (IpAddr::V6(_), IpAddr::V4(_)) => Ordering::Greater,
    }
}

fn sorted_nets<I>(nets: I) -> Vec<IpNet>
where
    I: IntoIterator<Item = IpNet>,
{
    let mut result = nets.into_iter().collect::<Vec<_>>();
    result.sort_by(|left, right| {
        ip_cmp(&left.addr(), &right.addr()).then(left.prefix_len().cmp(&right.prefix_len()))
    });
    result.dedup();
    result
}

/// Recover adapter names using the same listener policy, including when an
/// address is shared by selected, unselected, or tailnet adapters.
fn local_bindings(policy: &ListenPolicy, nics: &[Nic]) -> Vec<(IpAddr, String)> {
    bind_addrs(policy, nics)
        .into_iter()
        .filter_map(|(address, path)| {
            if !matches!(path, PathClass::Intranet | PathClass::Lan) {
                return None;
            }
            let nic = nics.iter().find(|nic| {
                bind_addrs(policy, std::slice::from_ref(nic)).contains(&(address, path))
            })?;
            Some((address.ip(), nic.name.clone()))
        })
        .collect()
}
