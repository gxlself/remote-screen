use std::net::IpAddr;

use host_core::listen::ListenPolicy;
use host_core::platform::Nic;
use host_win::firewall_model::*;
use ipnet::IpNet;
use serde_json::Value;

const EXE: &str = r"C:\Program Files\Remote Host\rs-host.exe";

fn ipnet(value: &str) -> IpNet {
    value.parse().expect("test CIDR")
}

fn ip(value: &str) -> IpAddr {
    value.parse().expect("test address")
}

fn nic(name: &str, addresses: &[&str], tailnet: bool) -> Nic {
    Nic {
        name: name.to_owned(),
        addresses: addresses.iter().map(|value| ip(value)).collect(),
        networks: Vec::new(),
        mac: None,
        tailnet,
    }
}

fn policy() -> ListenPolicy {
    ListenPolicy {
        tailnet: true,
        intranet_nics: vec!["以太网".to_owned()],
        allowed_cidrs: vec![ipnet("10.30.0.0/16"), ipnet("192.168.31.0/24")],
        lan_cidrs: vec![ipnet("192.168.31.0/24")],
        port: 7400,
    }
}

fn nics() -> Vec<Nic> {
    vec![
        nic("以太网", &["192.168.31.20", "fe80::1", "100.64.0.7"], false),
        nic("WLAN", &["192.168.1.5"], false),
        nic("Tailscale", &["100.64.0.7"], true),
    ]
}

#[test]
fn tailnet_and_intranet_rules_are_deterministic() {
    let tailnet = tailnet_rules(7400, Some(EXE));
    assert_eq!(tailnet.len(), 2);
    assert_eq!(tailnet[0].name, TAILNET_RULE_UDP);
    assert_eq!(tailnet[0].protocol, 17);
    assert_eq!(tailnet[1].name, TAILNET_RULE_TCP);
    assert_eq!(tailnet[1].protocol, 6);
    assert_eq!(tailnet[0].local_ports, "7400");
    assert_eq!(
        normalize_addresses(&tailnet[0].remote_addresses),
        [TAILNET_V4, TAILNET_V6]
    );
    assert_eq!(tailnet[0].profiles, PROFILE_ALL);
    assert_eq!(tailnet[0].application.as_deref(), Some(EXE));
    for rule in &tailnet {
        assert_eq!(rule.description, "Remote Host 经 tailnet 的入站连接");
        assert_eq!(rule.group, RULE_GROUP);
        assert_eq!(rule.local_addresses, "100.64.0.0/10,fd7a:115c:a1e0::/48");
        assert_eq!(rule.remote_addresses, rule.local_addresses);
        assert!(rule.enabled);
    }
    let custom_port = tailnet_rules(7500, None);
    assert!(custom_port.iter().all(|rule| rule.local_ports == "7500"));
    assert!(custom_port.iter().all(|rule| rule.application.is_none()));

    let intranet = intranet_rules(&policy(), &nics(), Some(EXE));
    assert_eq!(intranet.skipped, None);
    assert_eq!(intranet.rules.len(), 2);
    assert_eq!(intranet.rules[0].local_addresses, "192.168.31.20,fe80::1");
    assert_eq!(
        intranet.rules[0].remote_addresses,
        "10.30.0.0/16,192.168.31.0/24"
    );
    assert_eq!(intranet.rules[0].profiles, PROFILE_DOMAIN | PROFILE_PRIVATE);
    assert_eq!(intranet.rules[0].name, INTRANET_RULE_UDP);
    assert_eq!(intranet.rules[0].protocol, PROTO_UDP);
    assert_eq!(intranet.rules[1].name, INTRANET_RULE_TCP);
    assert_eq!(intranet.rules[1].protocol, PROTO_TCP);
    for rule in &intranet.rules {
        assert_eq!(rule.description, "Remote Host 内网 / 局域网入站连接");
        assert_eq!(rule.group, RULE_GROUP);
        assert_eq!(rule.local_ports, "7400");
        assert_eq!(rule.application.as_deref(), Some(EXE));
        assert!(rule.enabled);
    }

    let mut noncanonical = policy();
    noncanonical.allowed_cidrs = vec![ipnet("10.30.1.9/16")];
    assert_eq!(
        intranet_rules(&noncanonical, &nics(), None).rules[0].remote_addresses,
        "10.30.0.0/16"
    );
}

#[test]
fn intranet_skip_reasons_cover_each_empty_input() {
    let mut disabled = policy();
    disabled.intranet_nics.clear();
    let mut no_sources = policy();
    no_sources.allowed_cidrs.clear();
    let unknown_nic = vec![nic("WLAN", &["192.168.31.5"], false)];
    let only_tailnet = vec![nic("以太网", &["100.64.0.7", "fd7a:115c:a1e0::7"], false)];
    for (policy, nics, reason) in [
        (disabled, nics(), IntranetSkip::Disabled),
        (no_sources, nics(), IntranetSkip::NoSources),
        (policy(), unknown_nic, IntranetSkip::NoLocalAddresses),
        (policy(), only_tailnet, IntranetSkip::NoLocalAddresses),
    ] {
        let result = intranet_rules(&policy, &nics, None);
        assert_eq!(result.skipped, Some(reason));
        assert!(result.rules.is_empty());
    }
}

#[test]
fn intranet_addresses_and_sources_are_truncated_sorted_and_deduplicated() {
    let mut policy = policy();
    policy.port = 7500;
    policy.allowed_cidrs = vec![
        ipnet("fd7a:115c:a1e0::20/64"),
        ipnet("192.168.31.20/24"),
        ipnet("10.30.2.5/16"),
        ipnet("10.30.1.9/16"),
        ipnet("10.30.0.0/24"),
        ipnet("fe80::20/64"),
        ipnet("fe80::100/64"),
    ];
    let nics = vec![nic(
        "以太网",
        &[
            "fe80::10",
            "10.30.0.10",
            "10.30.0.2",
            "fe80::2",
            "10.30.0.10",
        ],
        false,
    )];
    let result = intranet_rules(&policy, &nics, None);
    assert_eq!(result.rules.len(), 2);
    for rule in result.rules {
        assert_eq!(rule.local_ports, "7500");
        assert_eq!(
            rule.local_addresses,
            "10.30.0.2,10.30.0.10,fe80::2,fe80::10"
        );
        assert_eq!(
            rule.remote_addresses,
            "10.30.0.0/16,10.30.0.0/24,192.168.31.0/24,fd7a:115c:a1e0::/64,fe80::/64"
        );
    }
}

#[test]
fn address_and_port_normalization_handles_windows_forms() {
    assert_eq!(
        normalize_addresses("192.168.31.20/255.255.255.255"),
        ["192.168.31.20/32"]
    );
    assert_eq!(
        normalize_addresses("10.30.0.0/255.255.0.0, 192.168.31.20"),
        ["10.30.0.0/16", "192.168.31.20/32"]
    );
    assert_eq!(normalize_addresses("10.30.5.5/16"), ["10.30.0.0/16"]);
    assert_eq!(
        normalize_addresses("FD7A:115C:A1E0::/48"),
        ["fd7a:115c:a1e0::/48"]
    );
    assert_eq!(normalize_addresses("fe80::1"), ["fe80::1/128"]);
    assert_eq!(normalize_addresses("*"), ["*"]);
    assert_eq!(normalize_addresses("Any"), ["*"]);
    assert_eq!(normalize_addresses("10.0.0.1,*"), ["*"]);
    assert_eq!(normalize_addresses("LocalSubnet"), ["localsubnet"]);
    assert_eq!(
        normalize_addresses("10.0.0.1-10.0.0.9"),
        ["10.0.0.1-10.0.0.9"]
    );
    assert_eq!(
        normalize_addresses("10.0.0.0/255.0.255.0"),
        ["10.0.0.0/255.0.255.0"]
    );
    assert!(normalize_addresses("").is_empty());
    assert_eq!(normalize_ports("7400, 80,7400"), ["80", "7400"]);
    assert_eq!(normalize_profiles(PROFILE_ALL), 7);
    assert_eq!(normalize_profiles(7), 7);
    assert_eq!(normalize_profiles(3), 3);
    assert_eq!(normalize_profiles(0x10003), 3);
}

#[test]
fn normalization_preserves_unparsed_values_and_has_stable_order() {
    assert_eq!(
        normalize_addresses(" fe80:0:0:0::1, , FE80::1/128, 192.168.31.20,192.168.31.20/32 "),
        ["192.168.31.20/32", "fe80::1/128"]
    );
    assert_eq!(
        normalize_addresses(
            "DNS, DHCP,WINS,DefaultGateway, Intranet,RmtIntranet,Internet,Ply2Renders,DNS"
        ),
        [
            "defaultgateway",
            "dhcp",
            "dns",
            "internet",
            "intranet",
            "ply2renders",
            "rmtintranet",
            "wins"
        ]
    );
    for (input, expected) in [
        ("10.30.1.9/255.255.0.0", "10.30.0.0/16"),
        ("10.30.1.9/0.0.0.0", "0.0.0.0/0"),
        ("FE80::10/64", "fe80::/64"),
        ("fe80::10/129", "fe80::10/129"),
        ("10.30.1.9/33", "10.30.1.9/33"),
        ("UnknownValue", "unknownvalue"),
    ] {
        assert_eq!(normalize_addresses(input), [expected]);
    }
    assert_eq!(
        normalize_ports(" , 7400, 10, 2, 80-90, RPC,10, , "),
        ["2", "10", "7400", "80-90", "RPC"]
    );
    assert_eq!(normalize_ports("7400,AnY"), ["*"]);
    assert_eq!(normalize_ports("7400,*"), ["*"]);
    assert!(normalize_ports(" , ").is_empty());
    assert_eq!(normalize_profiles(0), 0);
    assert_eq!(normalize_profiles(0x10007), 7);
}

#[test]
fn matching_and_plans_handle_masks_duplicates_and_unmanaged_rules() {
    let desired = intranet_rules(&policy(), &nics(), Some(EXE)).rules;
    let mut actual = desired.clone();
    actual[0].description = "different description".to_owned();
    actual[0].local_addresses = "192.168.31.20/255.255.255.255,fe80::1".to_owned();
    actual[0].remote_addresses = "10.30.0.0/255.255.0.0,192.168.31.0/255.255.255.0".to_owned();
    actual[0].application = Some(EXE.to_ascii_uppercase());
    assert!(rule_matches(&desired[0], &actual[0]));

    let mut mismatch = actual[0].clone();
    mismatch.profiles = PROFILE_ALL;
    assert!(!rule_matches(&desired[0], &mismatch));
    mismatch.profiles = desired[0].profiles;
    mismatch.application = None;
    assert!(!rule_matches(&desired[0], &mismatch));
    mismatch.application = desired[0].application.clone();
    mismatch.enabled = false;
    assert!(!rule_matches(&desired[0], &mismatch));
    mismatch.enabled = true;
    mismatch.group = "Other".to_owned();
    assert!(!rule_matches(&desired[0], &mismatch));

    let empty = plan(INTRANET_PREFIX, &desired, &[]);
    assert!(empty.remove_names.is_empty());
    assert_eq!(empty.add, desired);
    assert_eq!(plan(INTRANET_PREFIX, &desired, &desired), FwPlan::default());

    let mut different = desired.clone();
    different[0].remote_addresses = "10.30.0.0/24".to_owned();
    let changed = plan(INTRANET_PREFIX, &desired, &different);
    assert_eq!(changed.remove_names, vec![INTRANET_RULE_UDP]);
    assert_eq!(changed.add, vec![desired[0].clone()]);

    let duplicate = plan(
        INTRANET_PREFIX,
        &desired,
        &[desired[0].clone(), desired[1].clone(), desired[1].clone()],
    );
    assert_eq!(duplicate.remove_names, vec![INTRANET_RULE_TCP]);
    assert_eq!(duplicate.add, vec![desired[1].clone()]);

    let mut old = desired[0].clone();
    old.name = "Remote Host (内网) 旧".to_owned();
    let old_plan = plan(INTRANET_PREFIX, &desired, &[old]);
    assert_eq!(old_plan.remove_names, vec!["Remote Host (内网) 旧"]);
    assert_eq!(old_plan.add, desired);

    let unmanaged = plan(
        INTRANET_PREFIX,
        &desired,
        &[
            tailnet_rules(7400, None)[0].clone(),
            FwRuleSpec {
                name: "Other".to_owned(),
                ..desired[0].clone()
            },
        ],
    );
    assert!(unmanaged.remove_names.is_empty());
    assert_eq!(unmanaged.add, desired);
}

#[test]
fn rule_matching_checks_all_managed_fields() {
    let desired = intranet_rules(&policy(), &nics(), None).rules.remove(0);
    assert!(rule_matches(&desired, &desired));
    let mismatches = [
        FwRuleSpec {
            name: "Other".to_owned(),
            ..desired.clone()
        },
        FwRuleSpec {
            protocol: PROTO_TCP,
            ..desired.clone()
        },
        FwRuleSpec {
            local_ports: "7401".to_owned(),
            ..desired.clone()
        },
        FwRuleSpec {
            local_addresses: "192.168.31.21".to_owned(),
            ..desired.clone()
        },
        FwRuleSpec {
            remote_addresses: "10.99.0.0/24".to_owned(),
            ..desired.clone()
        },
    ];
    for actual in mismatches {
        assert!(!rule_matches(&desired, &actual), "{actual:?}");
    }
    let unicode_program = FwRuleSpec {
        application: Some(r"C:\Équipe\Remote Host\rs-host.exe".to_owned()),
        ..desired
    };
    let actual = FwRuleSpec {
        application: Some(r"c:\éQUIPE\REMOTE HOST\RS-HOST.EXE".to_owned()),
        ..unicode_program.clone()
    };
    assert!(rule_matches(&unicode_program, &actual));
}

#[test]
fn disabling_intranet_removes_every_managed_name_once_in_sorted_order() {
    let desired = intranet_rules(&ListenPolicy::default(), &nics(), None);
    let mut existing = intranet_rules(&policy(), &nics(), Some(EXE)).rules;
    existing.push(existing[0].clone());
    existing.push(FwRuleSpec {
        name: "Remote Host (内网) 旧".to_owned(),
        ..existing[0].clone()
    });
    existing.extend(tailnet_rules(7400, None));
    existing.push(FwRuleSpec {
        name: "Other".to_owned(),
        ..existing[0].clone()
    });
    assert_eq!(
        plan_intranet(&desired, &existing),
        FwPlan {
            remove_names: vec![
                INTRANET_RULE_TCP.to_owned(),
                INTRANET_RULE_UDP.to_owned(),
                "Remote Host (内网) 旧".to_owned(),
            ],
            add: Vec::new(),
        }
    );
}

#[test]
fn plans_preserve_desired_order_and_do_not_manage_other_prefixes() {
    let intranet = intranet_rules(&policy(), &nics(), Some(EXE)).rules;
    let tailnet = tailnet_rules(7400, None);
    let mut desired = intranet.clone();
    desired.extend(tailnet.clone());
    let mut existing = intranet.clone();
    existing[0].enabled = false;
    existing.push(intranet[0].clone());
    existing.push(intranet[1].clone());
    existing.extend(tailnet);
    let result = plan(INTRANET_PREFIX, &desired, &existing);
    assert_eq!(result.remove_names, [INTRANET_RULE_TCP, INTRANET_RULE_UDP]);
    assert_eq!(result.add, intranet);
}

#[test]
fn tailnet_status_and_netsh_commands_follow_contract() {
    let tailnet = tailnet_rules(7400, Some(EXE));
    assert!(tailnet_ok(&tailnet, 7400));
    assert!(!tailnet_ok(&tailnet[..1], 7400));
    let mut disabled = tailnet.clone();
    disabled[1].enabled = false;
    assert!(!tailnet_ok(&disabled, 7400));
    let mut mask = tailnet.clone();
    mask[1].remote_addresses = "100.64.0.0/255.192.0.0".to_owned();
    assert!(tailnet_ok(&mask, 7400));
    assert!(!tailnet_ok(&tailnet, 7401));

    let desired = intranet_rules(&policy(), &nics(), Some(EXE));
    assert_eq!(
        netsh_add_command(&desired.rules[0]),
        r#"netsh advfirewall firewall add rule name="Remote Host (内网) UDP" dir=in action=allow protocol=UDP localport=7400 localip=192.168.31.20,fe80::1 remoteip=10.30.0.0/16,192.168.31.0/24 profile=domain,private program="C:\Program Files\Remote Host\rs-host.exe" enable=yes"#
    );
    assert!(netsh_add_command(&tailnet[0]).contains("profile=any"));
    let mut without_program = desired.rules[0].clone();
    without_program.application = None;
    assert!(!netsh_add_command(&without_program).contains("program="));
}

#[test]
fn tailnet_check_accepts_msi_rules_without_local_addresses_group_or_program() {
    let mut rules = tailnet_rules(7400, None);
    for rule in &mut rules {
        rule.local_addresses = "*".to_owned();
        rule.group.clear();
        rule.profiles = PROFILE_PRIVATE;
        rule.local_ports = "80, 7400,7500".to_owned();
        rule.remote_addresses = "100.64.0.0/255.192.0.0".to_owned();
    }
    assert!(tailnet_ok(&rules, 7400));
    let mut invalid_duplicate = rules[1].clone();
    invalid_duplicate.enabled = false;
    rules.push(invalid_duplicate);
    assert!(tailnet_ok(&rules, 7400));
    rules[1].protocol = PROTO_UDP;
    assert!(!tailnet_ok(&rules, 7400));
    rules[1].protocol = PROTO_TCP;
    rules[1].remote_addresses = "*".to_owned();
    assert!(!tailnet_ok(&rules, 7400));
    rules[1].remote_addresses = TAILNET_V4.to_owned();
    rules[1].local_ports = "*".to_owned();
    assert!(!tailnet_ok(&rules, 7400));
}

#[test]
fn netsh_uses_any_for_wildcards_and_orders_profile_names() {
    let mut rule = tailnet_rules(7400, None).remove(1);
    rule.local_ports = "*".to_owned();
    rule.local_addresses = "*".to_owned();
    rule.remote_addresses = "Any".to_owned();
    rule.profiles = PROFILE_DOMAIN | PROFILE_PUBLIC | 0x10000;
    rule.enabled = false;
    assert_eq!(
        netsh_add_command(&rule),
        r#"netsh advfirewall firewall add rule name="Remote Host (tailnet) TCP" dir=in action=allow protocol=TCP localport=any localip=any remoteip=any profile=domain,public enable=no"#
    );
    rule.profiles = 7;
    assert!(netsh_add_command(&rule).contains("profile=any"));
}

#[test]
fn status_reports_profiles_rule_states_and_skips() {
    let desired = intranet_rules(&policy(), &nics(), Some(EXE));
    let facts = FwFacts {
        profiles: ProfileState {
            domain: false,
            private: true,
            public: false,
            current: PROFILE_PRIVATE | PROFILE_PUBLIC,
        },
        existing: desired.rules.clone(),
        ..FwFacts::default()
    };
    let result = status(&facts, &desired, 7400);
    assert!(result.enabled);
    assert!(result.active);
    assert_eq!(
        result.detail["current_profiles"],
        serde_json::json!(["private", "public"])
    );
    assert_eq!(result.detail["intranet"][0]["state"], "ok");
    assert_eq!(result.detail["intranet"][1]["state"], "ok");
    assert_eq!(result.detail["skipped"], Value::Null);

    let missing = status(
        &FwFacts {
            profiles: ProfileState {
                private: true,
                ..ProfileState::default()
            },
            existing: vec![desired.rules[0].clone()],
            ..FwFacts::default()
        },
        &desired,
        7400,
    );
    assert!(!missing.active);
    assert_eq!(missing.detail["intranet"][1]["state"], "missing");

    let skipped = IntranetPlan {
        rules: Vec::new(),
        skipped: Some(IntranetSkip::Disabled),
    };
    let skipped_result = status(
        &FwFacts {
            profiles: ProfileState::default(),
            policy_locked: true,
            error: Some("access denied".to_owned()),
            ..FwFacts::default()
        },
        &skipped,
        7400,
    );
    assert!(!skipped_result.enabled);
    assert!(skipped_result.active);
    assert_eq!(skipped_result.detail["skipped"], "disabled");
    assert_eq!(skipped_result.detail["policy_locked"], true);
    assert_eq!(skipped_result.detail["error"], "access denied");
}

#[test]
fn status_detail_has_the_fixed_it_rule_schema() {
    let desired = intranet_rules(&policy(), &nics(), Some(EXE));
    let mut existing = desired.rules.clone();
    existing.extend(tailnet_rules(7400, None));
    let facts = FwFacts {
        profiles: ProfileState {
            domain: true,
            private: true,
            public: false,
            current: PROFILE_PRIVATE,
        },
        existing,
        ..FwFacts::default()
    };
    assert_eq!(
        status(&facts, &desired, 7400).detail,
        serde_json::json!({
            "profiles": {"domain": true, "private": true, "public": false},
            "current_profiles": ["private"],
            "policy_locked": false,
            "tailnet": "ok",
            "intranet": [
                {"name": "Remote Host (内网) UDP", "state": "ok"},
                {"name": "Remote Host (内网) TCP", "state": "ok"}
            ],
            "skipped": null,
            "error": null,
            "rules": [
                {
                    "name": "Remote Host (内网) UDP",
                    "protocol": "UDP",
                    "local_port": "7400",
                    "local_addresses": "192.168.31.20,fe80::1",
                    "remote_addresses": "10.30.0.0/16,192.168.31.0/24",
                    "profiles": "domain,private",
                    "program": EXE
                },
                {
                    "name": "Remote Host (内网) TCP",
                    "protocol": "TCP",
                    "local_port": "7400",
                    "local_addresses": "192.168.31.20,fe80::1",
                    "remote_addresses": "10.30.0.0/16,192.168.31.0/24",
                    "profiles": "domain,private",
                    "program": EXE
                }
            ]
        })
    );
}

#[test]
fn status_counts_duplicates_and_keeps_profile_enablement_separate_from_rules() {
    let desired = intranet_rules(&policy(), &nics(), None);
    let mut facts = FwFacts {
        policy_locked: true,
        error: Some("E_ACCESSDENIED：访问被拒绝".to_owned()),
        existing: desired.rules.clone(),
        ..FwFacts::default()
    };
    facts.existing[0].enabled = false;
    facts.existing.push(facts.existing[1].clone());
    let result = status(&facts, &desired, 7400);
    assert!(!result.active);
    assert_eq!(
        result.detail["intranet"],
        serde_json::json!([
            {"name": "Remote Host (内网) UDP", "state": "mismatch"},
            {"name": "Remote Host (内网) TCP", "state": "duplicate"}
        ])
    );
    assert_eq!(result.detail["tailnet"], "missing");
    assert_eq!(result.detail["rules"][0]["program"], Value::Null);
    assert_eq!(result.detail["policy_locked"], true);
    assert_eq!(result.detail["error"], "E_ACCESSDENIED：访问被拒绝");
    facts.existing = desired.rules.clone();
    for (domain, private, public, enabled) in [
        (false, false, false, false),
        (false, false, true, false),
        (true, false, false, true),
        (false, true, false, true),
    ] {
        facts.profiles = ProfileState {
            domain,
            private,
            public,
            current: 0,
        };
        let result = status(&facts, &desired, 7400);
        assert_eq!(result.enabled, enabled);
        assert!(result.active);
    }
}

#[test]
fn status_uses_all_skip_tags_with_no_required_rules() {
    for (reason, tag) in [
        (IntranetSkip::Disabled, "disabled"),
        (IntranetSkip::NoSources, "no_sources"),
        (IntranetSkip::NoLocalAddresses, "no_local_addresses"),
    ] {
        let desired = IntranetPlan {
            rules: Vec::new(),
            skipped: Some(reason),
        };
        let result = status(&FwFacts::default(), &desired, 7400);
        assert!(result.active);
        assert_eq!(result.detail["skipped"], tag);
        assert_eq!(result.detail["rules"], serde_json::json!([]));
        assert_eq!(result.detail["intranet"], serde_json::json!([]));
    }
}

#[test]
fn it_note_describes_sources_addresses_policy_and_rules() {
    let text = it_note(&policy(), &nics(), Some(EXE), false);
    let lines = text.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 6);
    assert!(!text.ends_with('\n'));
    assert_eq!(lines[0], "Remote Host 使用 UDP 和 TCP 7400 端口。");
    assert_eq!(
        lines[1],
        "请允许来源网段 10.30.0.0/16 与 192.168.31.0/24 访问本机。"
    );
    assert_eq!(
        lines[2],
        "tailnet 地址：100.64.0.7；局域网地址：192.168.31.20（以太网）、fe80::1（以太网）。"
    );
    assert_eq!(
        lines[3],
        "如需手动添加，入站规则如下（仅「域」「专用」网络）："
    );
    let desired = intranet_rules(&policy(), &nics(), Some(EXE));
    assert_eq!(lines[4], netsh_add_command(&desired.rules[0]));
    assert_eq!(lines[5], netsh_add_command(&desired.rules[1]));

    let mut three = policy();
    three.allowed_cidrs.push(ipnet("10.99.0.0/24"));
    let three_text = it_note(&three, &nics(), None, true);
    assert!(three_text
        .contains("请允许来源网段 10.30.0.0/16、192.168.31.0/24 与 10.99.0.0/24 访问本机。"));
    assert!(three_text.contains("组策略"));
    assert_eq!(
        three_text.lines().nth(3),
        Some("这台电脑的防火墙由组策略管理，本机程序加不了规则，请在组策略里添加下面的入站规则（仅「域」「专用」网络）：")
    );

    let mut closed = policy();
    closed.intranet_nics.clear();
    assert!(it_note(&closed, &nics(), None, false)
        .ends_with("（内网连接未打开或没有可用的网卡地址，暂不需要规则）"));
}

#[test]
fn it_note_handles_empty_addresses_one_source_and_tailnet_disabled_policy() {
    let mut policy = policy();
    policy.tailnet = false;
    policy.allowed_cidrs = vec![ipnet("10.30.1.9/16")];
    let note = it_note(&policy, &nics(), None, false);
    assert_eq!(
        note.lines().nth(1),
        Some("请允许来源网段 10.30.0.0/16 访问本机。")
    );
    assert_eq!(
        note.lines().nth(2),
        Some("tailnet 地址：100.64.0.7；局域网地址：192.168.31.20（以太网）、fe80::1（以太网）。")
    );
    policy.allowed_cidrs.clear();
    let note = it_note(&policy, &[], None, false);
    assert_eq!(note.lines().nth(1), Some("还没有设置来源网段。"));
    assert_eq!(
        note.lines().nth(2),
        Some("tailnet 地址：无；局域网地址：无。")
    );
    assert_eq!(
        note.lines().last(),
        Some("（内网连接未打开或没有可用的网卡地址，暂不需要规则）")
    );
    assert!(!note.ends_with('\n'));
}

#[test]
fn it_note_uses_h11_binding_selection_to_find_adapter_names() {
    let mut policy = policy();
    policy.tailnet = false;
    let nics = vec![
        nic("WLAN", &["192.168.31.20"], false),
        nic("Tailscale", &["192.168.31.20"], true),
        nic("以太网", &["192.168.31.20"], false),
    ];
    assert_eq!(
        it_note(&policy, &nics, None, false).lines().nth(2),
        Some("tailnet 地址：192.168.31.20；局域网地址：192.168.31.20（以太网）。")
    );
    assert_eq!(
        intranet_rules(&policy, &nics, None).rules[0].local_addresses,
        "192.168.31.20"
    );
    policy.tailnet = true;
    assert_eq!(
        intranet_rules(&policy, &nics, None).skipped,
        Some(IntranetSkip::NoLocalAddresses)
    );
    assert_eq!(
        it_note(&policy, &nics, None, false).lines().nth(2),
        Some("tailnet 地址：192.168.31.20；局域网地址：无。")
    );
}
