use std::collections::BTreeSet;

use rs_mobile::rscore::dto_core::{
    SshAuth, SshServer, SshSource, SshTarget as SshTargetDto, TrustedHostKey,
};
use rs_mobile::rscore::sshbook::{
    entry_to_row, parse_host_port, parse_servers_value, resolve, row_to_entry, ssh_servers,
    tmux_id, trusted_host_keys, BookError, ManualServer, ResolvedTarget,
};
use rs_mobile::rscore::store::KnownHostRow;
use rs_ssh::{tmux, HostKeyEntry};
use serde_json::{json, Value};

fn manual_server_json(id: &str, jump: Value, auth: &str) -> Value {
    json!({
        "id": id,
        "kind": "ssh",
        "name": "build-srv",
        "host": "10.30.20.31",
        "port": 22,
        "user": "alice",
        "auth": auth,
        "jump": jump,
        "networkId": "office",
        "addedAtUs": 10,
        "updatedAtUs": 20
    })
}

fn server(
    server_id: &str,
    name: &str,
    host: &str,
    port: u16,
    user: &str,
    jump: Option<&str>,
) -> SshServer {
    SshServer {
        server_id: server_id.into(),
        name: name.into(),
        host: host.into(),
        port,
        user: user.into(),
        jump: jump.map(str::to_owned),
        auth: SshAuth::Password,
        tailscale_ssh: None,
        os: None,
        source: SshSource::Manual,
    }
}

#[test]
fn parses_servers_value_leniently() {
    for value in [
        None,
        Some(json!("x")),
        Some(json!({"v": 2})),
        Some(json!({"v": 1})),
    ] {
        let parsed = parse_servers_value(value.as_ref());
        assert!(parsed.manual.is_empty());
        assert!(parsed.tailscale_ssh.is_empty());
    }

    let value = json!({
        "v": 1,
        "entries": [
            manual_server_json("manual-build-srv", json!("jump-01"), "password"),
            {
                "id": "missing-host",
                "kind": "ssh",
                "name": "missing-host",
                "port": 22,
                "user": "alice",
                "auth": "deviceKey",
                "jump": null,
                "networkId": null
            },
            {
                "id": "computer-1",
                "kind": "computer",
                "host": "192.168.31.20",
                "port": 7400
            },
            manual_server_json("bad-auth", Value::Null, "rsa")
        ],
        "tailscaleSsh": {"a": true, "b": "yes"}
    });
    let parsed = parse_servers_value(Some(&value));

    assert_eq!(parsed.manual.len(), 1);
    assert_eq!(
        parsed.manual[0],
        ManualServer {
            id: "manual-build-srv".into(),
            name: "build-srv".into(),
            host: "10.30.20.31".into(),
            port: 22,
            user: "alice".into(),
            auth: SshAuth::Password,
            jump: Some("jump-01".into()),
            network_id: Some("office".into()),
        }
    );
    assert_eq!(parsed.tailscale_ssh.len(), 1);
    assert_eq!(parsed.tailscale_ssh.get("a"), Some(&true));

    let integer_version = json!({
        "v": 1,
        "entries": [manual_server_json("manual-build-srv", Value::Null, "password")],
        "tailscaleSsh": {}
    });
    assert_eq!(parse_servers_value(Some(&integer_version)).manual.len(), 1);

    let bad_ports = json!({
        "v": 1,
        "entries": [
            {"id": "zero", "kind": "ssh", "name": "zero", "host": "10.30.0.1", "port": 0, "user": "u", "auth": "password"},
            {"id": "wide", "kind": "ssh", "name": "wide", "host": "10.30.0.2", "port": 65536, "user": "u", "auth": "password"}
        ],
        "tailscaleSsh": {}
    });
    assert!(parse_servers_value(Some(&bad_ports)).manual.is_empty());
}

#[test]
fn lists_manual_servers_with_typescript_shape() {
    let value = json!({
        "v": 1,
        "entries": [manual_server_json("manual-build-srv", json!("jump-01"), "password")],
        "tailscaleSsh": {"gpu-train-02": true}
    });
    let servers = parse_servers_value(Some(&value));
    let listed = ssh_servers(&servers);

    assert_eq!(
        listed
            .iter()
            .map(|server| server.server_id.as_str())
            .collect::<Vec<_>>(),
        ["manual-build-srv"]
    );

    let manual = &listed[0];
    assert_eq!(manual.source, SshSource::Manual);
    assert_eq!(manual.jump.as_deref(), Some("jump-01"));
    assert_eq!(manual.tailscale_ssh, None);

    let manual_json = serde_json::to_value(manual).expect("serialize manual server");
    assert_eq!(
        manual_json,
        json!({
            "serverId": "manual-build-srv",
            "name": "build-srv",
            "host": "10.30.20.31",
            "port": 22,
            "user": "alice",
            "jump": "jump-01",
            "auth": "password",
            "tailscaleSsh": null,
            "source": "manual"
        })
    );
    let manual_keys: BTreeSet<_> = manual_json
        .as_object()
        .expect("server object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        manual_keys,
        BTreeSet::from([
            "serverId",
            "name",
            "host",
            "port",
            "user",
            "jump",
            "auth",
            "tailscaleSsh",
            "source",
        ])
    );
    assert_eq!(manual_json["os"], Value::Null);
}

#[test]
fn resolves_targets_and_one_jump_hop() {
    let target = server(
        "manual-build-srv",
        "build-srv",
        "10.30.20.31",
        22,
        "alice",
        Some("jump-01"),
    );

    let with_catalog_jump = [
        target.clone(),
        server("jump-01", "jump-01", "10.30.0.5", 22, "ops", None),
    ];
    let resolved = resolve("manual-build-srv", &with_catalog_jump, None).expect("resolve target");
    let jump = resolved.target.jump.as_deref().expect("catalog jump");
    assert_eq!(jump.host, "10.30.0.5");
    assert_eq!(jump.port, 22);
    assert_eq!(jump.user, "ops");

    let without_catalog_jump = [target.clone()];
    let resolved =
        resolve("manual-build-srv", &without_catalog_jump, None).expect("resolve textual jump");
    let jump = resolved.target.jump.as_deref().expect("textual jump");
    assert_eq!(jump.host, "jump-01");
    assert_eq!(jump.port, 22);
    assert_eq!(jump.user, "alice");

    let port_target = SshServer {
        jump: Some("10.30.0.5:2222".into()),
        ..target.clone()
    };
    let resolved = resolve("manual-build-srv", &[port_target], None).expect("resolve jump port");
    assert_eq!(
        resolved
            .target
            .jump
            .as_deref()
            .expect("jump with explicit port")
            .port,
        2222
    );

    let bad_jump = SshServer {
        jump: Some("a b".into()),
        ..target.clone()
    };
    assert_eq!(
        resolve("manual-build-srv", &[bad_jump], None),
        Err(BookError::BadJump("a b".into()))
    );

    assert_eq!(
        resolve("unknown", &with_catalog_jump, None),
        Err(BookError::UnknownServer("unknown".into()))
    );

    let jump_with_jump = SshServer {
        jump: Some("10.30.0.9:22".into()),
        ..server("jump-01", "jump-01", "10.30.0.5", 22, "ops", None)
    };
    let resolved = resolve("manual-build-srv", &[target.clone(), jump_with_jump], None)
        .expect("ignore nested jump");
    assert_eq!(
        resolved.target.jump.as_deref().expect("one jump").jump,
        None
    );

    let bad_port = SshServer {
        port: 0,
        ..target.clone()
    };
    assert_eq!(
        resolve("manual-build-srv", &[bad_port], None),
        Err(BookError::BadTarget("port is zero"))
    );
    let bad_host = SshServer {
        host: String::new(),
        ..target.clone()
    };
    assert_eq!(
        resolve("manual-build-srv", &[bad_host], None),
        Err(BookError::BadTarget("host is empty"))
    );
    let bad_user = SshServer {
        user: String::new(),
        ..target
    };
    assert_eq!(
        resolve("manual-build-srv", &[bad_user], None),
        Err(BookError::BadTarget("user is empty"))
    );
}

#[test]
fn resolves_override_targets_and_rejects_invalid_fields() {
    let override_target = SshTargetDto {
        host: "10.30.0.9".into(),
        port: 22,
        user: "root".into(),
        jump: None,
        auth: SshAuth::Password,
    };
    assert_eq!(
        resolve("not-in-book", &[], Some(&override_target)),
        Ok(ResolvedTarget {
            server_id: "not-in-book".into(),
            target: rs_ssh::SshTarget {
                host: "10.30.0.9".into(),
                port: 22,
                user: "root".into(),
                jump: None,
            },
            auth: SshAuth::Password,
            tailscale_ssh: None,
        })
    );

    let empty_host = SshTargetDto {
        host: String::new(),
        ..override_target.clone()
    };
    assert_eq!(
        resolve("bad", &[], Some(&empty_host)),
        Err(BookError::BadTarget("host is empty"))
    );
    let zero_port = SshTargetDto {
        port: 0,
        ..override_target.clone()
    };
    assert_eq!(
        resolve("bad", &[], Some(&zero_port)),
        Err(BookError::BadTarget("port is zero"))
    );
    let empty_user = SshTargetDto {
        user: String::new(),
        ..override_target
    };
    assert_eq!(
        resolve("bad", &[], Some(&empty_user)),
        Err(BookError::BadTarget("user is empty"))
    );
}

#[test]
fn parses_bracketed_ipv6_and_rejects_bad_ports() {
    assert_eq!(
        parse_host_port("[fd7a:115c:a1e0::7]:22", 2200),
        Some(("fd7a:115c:a1e0::7".into(), 22))
    );
    assert_eq!(parse_host_port("jump-01", 22), Some(("jump-01".into(), 22)));
    assert_eq!(
        parse_host_port("10.30.0.5:2222", 22),
        Some(("10.30.0.5".into(), 2222))
    );
    assert_eq!(parse_host_port("h:99999", 22), None);
    assert_eq!(parse_host_port("", 22), None);
    assert_eq!(parse_host_port("h:0", 22), None);
    assert_eq!(parse_host_port("h:22:33", 22), None);
    assert_eq!(parse_host_port("[jump-01]:22", 22), None);
    assert_eq!(parse_host_port("[2001:db8::1", 22), None);
}

#[test]
fn converts_known_hosts_rows_and_trusted_keys() {
    let entry = HostKeyEntry {
        algorithm: "ssh-ed25519".into(),
        key: vec![1, 2, 3, 4],
        fingerprint: "SHA256:test".into(),
    };
    let row = entry_to_row("s1", "10.30.0.5", 22, &entry, 5);
    assert_eq!(row_to_entry(&row), entry);
    assert_eq!(row.server_id, "s1");
    assert_eq!(row.host, "10.30.0.5");
    assert_eq!(row.port, 22);
    assert_eq!(row.added_at_us, 5);

    let later_row = KnownHostRow {
        server_id: "s2".into(),
        fingerprint: "SHA256:later".into(),
        added_at_us: 9,
        ..row.clone()
    };
    let keys: Vec<TrustedHostKey> = trusted_host_keys(&[later_row, row]);
    assert_eq!(
        serde_json::to_value(&keys).expect("serialize trusted keys"),
        json!([
            {
                "serverId": "s1",
                "keyType": "ssh-ed25519",
                "fp": "SHA256:test",
                "addedAt": 5
            },
            {
                "serverId": "s2",
                "keyType": "ssh-ed25519",
                "fp": "SHA256:later",
                "addedAt": 9
            }
        ])
    );
}

#[test]
fn creates_safe_tmux_session_ids() {
    assert_eq!(tmux_id("prod-api-01"), "prod-api-01");
    assert_eq!(tmux_id("manual.build srv"), "manual-build-srv");
    assert_eq!(tmux_id(""), "server");

    let first = tmux_id(&"a".repeat(40));
    let second = tmux_id(&"b".repeat(40));
    assert_eq!(first.len(), 32);
    assert_eq!(second.len(), 32);
    assert_ne!(first, second);

    for id in [
        "prod-api-01".to_owned(),
        "manual-build-srv".to_owned(),
        first,
        second,
        tmux_id(""),
    ] {
        assert!(tmux::command(&id).is_ok(), "invalid tmux id {id}");
    }
}
