use rs_mobile::rscore::dto_core::{DevicePlatform, EmbeddedNodeSummary, TailnetRoute};
use rs_mobile::rscore::tailnet_policy::{
    decide, node_hostname, phase_of, route_status, Decision, NodePhase, PolicyInput, StopWhen,
    TailnetMode,
};

fn policy(mode: TailnetMode) -> PolicyInput {
    PolicyInput {
        mode,
        available: true,
        control_url_set: true,
        system_tailnet_up: false,
        app_active: true,
        woken: false,
        login_pending: false,
        live_connections: false,
        node_started: false,
    }
}

#[test]
fn mode_parse_roundtrip() {
    for (value, mode) in [
        ("system", TailnetMode::System),
        ("embedded", TailnetMode::Embedded),
        ("auto", TailnetMode::Auto),
    ] {
        assert_eq!(TailnetMode::parse(value), Some(mode));
        assert_eq!(mode.as_str(), value);
    }
    for value in ["System", "", "on"] {
        assert_eq!(TailnetMode::parse(value), None, "{value:?}");
    }
}

#[test]
fn phase_table() {
    for (state, phase) in [
        ("running", NodePhase::Running),
        ("needsLogin", NodePhase::NeedsLogin),
        ("needsMachineAuth", NodePhase::NeedsLogin),
        ("noState", NodePhase::Starting),
        ("starting", NodePhase::Starting),
        ("stopped", NodePhase::Down),
        ("unknown", NodePhase::Down),
        ("unavailable", NodePhase::Down),
        ("whatever", NodePhase::Down),
    ] {
        assert_eq!(phase_of(state), phase, "{state}");
    }
}

struct Row {
    input: PolicyInput,
    route: TailnetRoute,
    want_node: bool,
    stop: StopWhen,
}

#[test]
fn decide_table() {
    let rows = [
        Row {
            input: policy(TailnetMode::System),
            route: TailnetRoute::System,
            want_node: false,
            stop: StopWhen::Never,
        },
        Row {
            input: PolicyInput {
                node_started: true,
                ..policy(TailnetMode::System)
            },
            route: TailnetRoute::System,
            want_node: false,
            stop: StopWhen::Now,
        },
        Row {
            input: PolicyInput {
                node_started: true,
                live_connections: true,
                ..policy(TailnetMode::System)
            },
            route: TailnetRoute::System,
            want_node: true,
            stop: StopWhen::Never,
        },
        Row {
            input: policy(TailnetMode::Embedded),
            route: TailnetRoute::Embedded,
            want_node: true,
            stop: StopWhen::Never,
        },
        Row {
            input: PolicyInput {
                control_url_set: false,
                ..policy(TailnetMode::Embedded)
            },
            route: TailnetRoute::System,
            want_node: false,
            stop: StopWhen::Never,
        },
        Row {
            input: PolicyInput {
                available: false,
                node_started: true,
                ..policy(TailnetMode::Embedded)
            },
            route: TailnetRoute::System,
            want_node: false,
            stop: StopWhen::Now,
        },
        Row {
            input: PolicyInput {
                app_active: false,
                node_started: true,
                ..policy(TailnetMode::Embedded)
            },
            route: TailnetRoute::Embedded,
            want_node: false,
            stop: StopWhen::AfterGrace,
        },
        Row {
            input: PolicyInput {
                app_active: false,
                node_started: true,
                live_connections: true,
                ..policy(TailnetMode::Embedded)
            },
            route: TailnetRoute::Embedded,
            want_node: true,
            stop: StopWhen::Never,
        },
        Row {
            input: PolicyInput {
                app_active: false,
                woken: true,
                ..policy(TailnetMode::Embedded)
            },
            route: TailnetRoute::Embedded,
            want_node: true,
            stop: StopWhen::Never,
        },
        Row {
            input: PolicyInput {
                app_active: false,
                node_started: true,
                login_pending: true,
                ..policy(TailnetMode::Embedded)
            },
            route: TailnetRoute::Embedded,
            want_node: true,
            stop: StopWhen::Never,
        },
        Row {
            input: PolicyInput {
                app_active: false,
                ..policy(TailnetMode::Embedded)
            },
            route: TailnetRoute::Embedded,
            want_node: false,
            stop: StopWhen::Never,
        },
        Row {
            input: PolicyInput {
                system_tailnet_up: true,
                ..policy(TailnetMode::Auto)
            },
            route: TailnetRoute::System,
            want_node: false,
            stop: StopWhen::Never,
        },
        Row {
            input: policy(TailnetMode::Auto),
            route: TailnetRoute::Embedded,
            want_node: true,
            stop: StopWhen::Never,
        },
        Row {
            input: PolicyInput {
                system_tailnet_up: true,
                node_started: true,
                ..policy(TailnetMode::Auto)
            },
            route: TailnetRoute::System,
            want_node: false,
            stop: StopWhen::Now,
        },
        Row {
            input: PolicyInput {
                system_tailnet_up: true,
                node_started: true,
                live_connections: true,
                ..policy(TailnetMode::Auto)
            },
            route: TailnetRoute::System,
            want_node: true,
            stop: StopWhen::Never,
        },
        Row {
            input: PolicyInput {
                system_tailnet_up: true,
                node_started: true,
                login_pending: true,
                ..policy(TailnetMode::Auto)
            },
            route: TailnetRoute::System,
            want_node: false,
            stop: StopWhen::Now,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        assert_eq!(
            decide(&row.input),
            Decision {
                route: row.route,
                want_node: row.want_node,
                stop: row.stop,
            },
            "row {}",
            index + 1
        );
    }
}

#[test]
fn route_status_fields() {
    let summary = EmbeddedNodeSummary {
        state: "running".into(),
        hostname: "rs-ios-k7m2q4xa".into(),
        ips: vec!["100.64.0.21".into()],
        derp_region: "sha".into(),
        derp_region_name: "Shanghai".into(),
    };
    let status = route_status(
        TailnetMode::Auto,
        TailnetRoute::Embedded,
        true,
        true,
        Some(summary),
    );
    let value = serde_json::to_value(status).unwrap();

    assert_eq!(value["mode"], "auto");
    assert_eq!(value["active"], "embedded");
    assert_eq!(value["embedded"]["state"], "running");
}

#[test]
fn hostname_table() {
    for (platform, device_id, expected) in [
        (
            DevicePlatform::Ios,
            Some("K7M2Q4XAB9C3D5EF"),
            "rs-ios-k7m2q4xa",
        ),
        (DevicePlatform::Ipados, Some("abcd2345"), "rs-ipad-abcd2345"),
        (DevicePlatform::Android, Some("ab-c"), "rs-android-app"),
        (DevicePlatform::Ios, None, "rs-ios-app"),
    ] {
        assert_eq!(node_hostname(platform, device_id), expected);
    }
}
