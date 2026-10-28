use std::collections::BTreeMap;

use rs_mobile::rscore::dto_core::*;
use rsp_client::EndReason;
use rsp_core::pb;
use rsp_transport::dial::PathKind as DialPath;
use serde_json::{json, Value};

fn participant() -> Participant {
    Participant {
        device_id: "d1".into(),
        device_name: "iPad".into(),
        user: "me@example.com".into(),
        path: Some(NetPath::Lan),
        since: 5,
    }
}

#[test]
fn discover_probe_omits_absent_banner_and_round_trips_outcomes() {
    for (outcome, wire) in [
        (DiscoverOutcome::Open, "open"),
        (DiscoverOutcome::Mismatch, "mismatch"),
        (DiscoverOutcome::Refused, "refused"),
        (DiscoverOutcome::Timeout, "timeout"),
    ] {
        let mut probe = DiscoverProbe {
            addr: "192.168.31.20".into(),
            port: 5900,
            outcome,
            ms: 800,
            banner: None,
        };
        let value = serde_json::to_value(&probe).expect("probe JSON");
        assert_eq!(
            value,
            json!({"addr": "192.168.31.20", "port": 5900, "outcome": wire, "ms": 800})
        );
        assert_eq!(
            serde_json::from_value::<DiscoverProbe>(value).expect("probe"),
            probe
        );
        probe.banner = Some("RFB 003.889".into());
        let value = serde_json::to_value(&probe).expect("probe banner JSON");
        assert_eq!(value["banner"], "RFB 003.889");
        assert_eq!(
            serde_json::from_value::<DiscoverProbe>(value).expect("probe"),
            probe
        );
    }
}

#[test]
fn address_pairing_arguments_require_an_address() {
    let with_port: PairStartArg = serde_json::from_value(json!({
        "addr": "100.64.0.3", "port": 7400, "code": "123456"
    }))
    .expect("address pairing arg");
    assert!(matches!(
        with_port,
        PairStartArg::CodeAddr {
            port: Some(7400),
            ..
        }
    ));
    let without_port: PairStartArg = serde_json::from_value(json!({
        "addr": "studio-mac", "code": "654321"
    }))
    .expect("address pairing arg without port");
    assert!(matches!(
        without_port,
        PairStartArg::CodeAddr { port: None, .. }
    ));
    // Pairing requires an explicit Host address.
    assert!(serde_json::from_value::<PairStartArg>(json!({
        "hostId": "host", "code": "123456"
    }))
    .is_err());
}

#[test]
fn paired_host_v11_fields_use_camel_case_and_defaults() {
    let from_legacy: PairedHost = serde_json::from_value(json!({
        "hostId": "h1", "fp": "fp", "name": "Host", "addresses": [], "pairedAt": 1
    }))
    .expect("pre-V11 paired host JSON");
    let expected = PairedHost {
        host_id: "h1".into(),
        fp: "fp".into(),
        name: "Host".into(),
        addresses: vec![],
        paired_at: 1,
        os: None,
        os_version: None,
        host_version: None,
        features: None,
        displays: vec![],
        last_seen_at: None,
    };
    assert_eq!(from_legacy, expected);
    let value = serde_json::to_value(expected).expect("paired host JSON");
    assert_eq!(value["os"], Value::Null);
    assert_eq!(value["osVersion"], Value::Null);
    assert_eq!(value["hostVersion"], Value::Null);
    assert_eq!(value["features"], Value::Null);
    assert_eq!(value["displays"], json!([]));
    assert_eq!(value["lastSeenAt"], Value::Null);
    assert_eq!(
        serde_json::from_value::<PairedHost>(value).expect("explicit null and empty defaults"),
        from_legacy
    );
}

#[test]
fn discover_events_use_camel_case_and_decimal_scan_ids() {
    let scan_id = id_str(u64::MAX);
    let events = [
        (
            CoreEvent::DiscoverFound {
                scan_id: scan_id.clone(),
                addr: "192.168.31.20".into(),
                port: 5900,
                banner: None,
            },
            json!({"type": "discover.found", "scanId": scan_id, "addr": "192.168.31.20", "port": 5900}),
        ),
        (
            CoreEvent::DiscoverFound {
                scan_id: scan_id.clone(),
                addr: "192.168.31.20".into(),
                port: 5900,
                banner: Some("RFB 003.889".into()),
            },
            json!({"type": "discover.found", "scanId": scan_id, "addr": "192.168.31.20", "port": 5900, "banner": "RFB 003.889"}),
        ),
        (
            CoreEvent::DiscoverDone {
                scan_id: scan_id.clone(),
                scanned: 254,
                cancelled: true,
            },
            json!({"type": "discover.done", "scanId": scan_id, "scanned": 254, "cancelled": true}),
        ),
    ];
    for (event, expected) in events {
        let value = serde_json::to_value(&event).expect("event JSON");
        assert_eq!(value, expected);
        assert_eq!(value["type"], event.event_type());
        assert_eq!(
            serde_json::from_value::<CoreEvent>(value).expect("event"),
            event
        );
    }
}

#[tokio::test]
async fn discovery_exports_are_inert_stubs() {
    use rs_mobile::rscore::api_discover::{
        discover_probe, discover_scan_cancel, discover_scan_lan,
    };
    use rs_mobile::rscore::error::CoreError;

    assert!(matches!(
        discover_probe("192.168.31.20".into(), 5900, "vnc".into(), 800).await,
        Err(CoreError::Unsupported { message }) if message == "discover: not built yet"
    ));
    assert!(matches!(
        discover_scan_lan(vec![5900, 3389]).await,
        Err(CoreError::Unsupported { message }) if message == "discover: not built yet"
    ));
    discover_scan_cancel(u64::MAX);
}

fn event_samples() -> Vec<(CoreEvent, Value)> {
    vec![
        (
            CoreEvent::SessionState {
                session_id: id_str(7),
                host_id: "h1".into(),
                phase: SessionPhase::PendingApproval,
                path: None,
                transport: None,
                derp_region: None,
                dial: None,
                approval_timeout_s: Some(30),
                resumed: None,
                reconnect_reason: None,
            },
            json!({
                "type": "session.state",
                "sessionId": "7",
                "hostId": "h1",
                "phase": "pendingApproval",
                "approvalTimeoutS": 30
            }),
        ),
        (
            CoreEvent::SessionRole {
                session_id: id_str(7),
                role: Role::Controller,
                controller: None,
                viewers: vec![],
                input_paused_local: false,
            },
            json!({
                "type": "session.role",
                "sessionId": "7",
                "role": "controller",
                "controller": null,
                "viewers": [],
                "inputPausedLocal": false
            }),
        ),
        (
            CoreEvent::SessionStats {
                session_id: id_str(7),
                rtt_ms: 6,
                fps: 60,
                bitrate_kbps: 8_000,
                decode_ms: 4.0,
                loss_pct: 0.5,
                width: 2_560,
                height: 1_600,
                codec: Codec::Hevc,
                path: NetPath::Intranet,
                transport: Transport::Quic,
                derp_region: Some("Shanghai".into()),
                rate_reason: None,
            },
            json!({
                "type": "session.stats",
                "sessionId": "7",
                "rttMs": 6,
                "fps": 60,
                "bitrateKbps": 8000,
                "decodeMs": 4.0,
                "lossPct": 0.5,
                "width": 2560,
                "height": 1600,
                "codec": "hevc",
                "path": "intranet",
                "transport": "quic",
                "derpRegion": "Shanghai"
            }),
        ),
        (
            CoreEvent::SessionError {
                session_id: id_str(7),
                host_id: "h1".into(),
                code: Some(101),
                reason: Some(GoodbyeReason::PermissionLost),
                message: Some("missing screen recording".into()),
                detail: Some(BTreeMap::from([(
                    "host_version".to_owned(),
                    "0.2.4".to_owned(),
                )])),
                retry_after_s: Some(5),
            },
            json!({
                "type": "session.error",
                "sessionId": "7",
                "hostId": "h1",
                "code": 101,
                "reason": "permissionLost",
                "message": "missing screen recording",
                "detail": {"host_version": "0.2.4"},
                "retryAfterS": 5
            }),
        ),
        (
            CoreEvent::SessionDisplays {
                session_id: id_str(7),
                displays: vec![SessionDisplay {
                    id: 1,
                    name: "Built-in".into(),
                    width_px: 2_560,
                    height_px: 1_600,
                    primary: true,
                    refresh_hz: Some(120),
                    is_virtual: false,
                }],
                current_id: 1,
            },
            json!({
                "type": "session.displays",
                "sessionId": "7",
                "displays": [{
                    "id": 1,
                    "name": "Built-in",
                    "widthPx": 2560,
                    "heightPx": 1600,
                    "primary": true,
                    "refreshHz": 120
                }],
                "currentId": 1
            }),
        ),
        (
            CoreEvent::SessionClipboard {
                session_id: id_str(7),
                direction: ClipboardDirection::FromHost,
                text: "hello".into(),
                at: 42,
            },
            json!({
                "type": "session.clipboard",
                "sessionId": "7",
                "direction": "fromHost",
                "text": "hello",
                "at": 42
            }),
        ),
        (
            CoreEvent::SessionIdleWarning {
                session_id: id_str(7),
                seconds_left: 20,
                lock_on_end: true,
            },
            json!({
                "type": "session.idleWarning",
                "sessionId": "7",
                "secondsLeft": 20,
                "lockOnEnd": true
            }),
        ),
        (
            CoreEvent::SessionControlRequest {
                session_id: id_str(7),
                requester: participant(),
                timeout_s: 30,
            },
            json!({
                "type": "session.controlRequest",
                "sessionId": "7",
                "requester": {
                    "deviceId": "d1",
                    "deviceName": "iPad",
                    "user": "me@example.com",
                    "path": "lan",
                    "since": 5
                },
                "timeoutS": 30
            }),
        ),
        (
            CoreEvent::PairingProgress {
                step: PairStep::WaitingLocal,
                host_name: Some("Mac".into()),
                path: Some(NetPath::TailnetDirect),
                latency_ms: Some(12),
                fp: Some("fp".into()),
                timeout_s: Some(30),
            },
            json!({
                "type": "pairing.progress",
                "step": "waitingLocal",
                "hostName": "Mac",
                "path": "tailnetDirect",
                "latencyMs": 12,
                "fp": "fp",
                "timeoutS": 30
            }),
        ),
        (
            CoreEvent::PairingDone {
                outcome: PairOutcome::BadCode { attempts_left: 3 },
            },
            json!({
                "type": "pairing.done",
                "outcome": {"status": "badCode", "attemptsLeft": 3}
            }),
        ),
        (
            CoreEvent::ProbeResult {
                probe_id: id_str(9),
                host_id: None,
                path: NetPath::Lan,
                addr: "10.30.0.5".into(),
                port: 7_400,
                transport: Transport::Tcp,
                outcome: ProbeOutcome::Ok,
                ms: None,
                reason: None,
                last: true,
            },
            json!({
                "type": "probe.result",
                "probeId": "9",
                "hostId": null,
                "path": "lan",
                "addr": "10.30.0.5",
                "port": 7400,
                "transport": "tcp",
                "outcome": "ok",
                "ms": null,
                "last": true
            }),
        ),
        (
            CoreEvent::SshState {
                ssh_id: id_str(9),
                server_id: "prod-api-01".into(),
                state: SshState::Open,
                message: Some("ready".into()),
            },
            json!({
                "type": "ssh.state",
                "sshId": "9",
                "serverId": "prod-api-01",
                "state": "open",
                "message": "ready"
            }),
        ),
        (
            CoreEvent::SshHostKeyPrompt {
                ssh_id: id_str(9),
                server_id: "prod-api-01".into(),
                key_type: "ssh-ed25519".into(),
                fp: "SHA256:new".into(),
                kind: HostKeyPromptKind::Changed,
                old_fp: Some("SHA256:old".into()),
            },
            json!({
                "type": "ssh.hostKeyPrompt",
                "sshId": "9",
                "serverId": "prod-api-01",
                "keyType": "ssh-ed25519",
                "fp": "SHA256:new",
                "kind": "changed",
                "oldFp": "SHA256:old"
            }),
        ),
        (
            CoreEvent::SshAuthUrl {
                ssh_id: id_str(9),
                server_id: "prod-api-01".into(),
                url: "https://hs.example.com/check".into(),
                user: "me".into(),
            },
            json!({
                "type": "ssh.authUrl",
                "sshId": "9",
                "serverId": "prod-api-01",
                "url": "https://hs.example.com/check",
                "user": "me"
            }),
        ),
        (
            CoreEvent::NetworkChanged {
                status: NetworkStatus {
                    ifaces: vec![NetIface {
                        name: "en0".into(),
                        addrs: vec!["192.168.31.20".into()],
                        is_vpn: false,
                        work_profile: Some(true),
                    }],
                    tailnet: None,
                },
            },
            json!({
                "type": "network.changed",
                "status": {
                    "ifaces": [{
                        "name": "en0",
                        "addrs": ["192.168.31.20"],
                        "isVpn": false,
                        "workProfile": true
                    }]
                }
            }),
        ),
    ]
}

#[test]
fn network_status_tailnet_round_trips_with_camel_case_fields() {
    let mut status = NetworkStatus {
        ifaces: vec![],
        tailnet: Some(TailnetRouteStatus {
            mode: "embedded".into(),
            active: TailnetRoute::Embedded,
            available: true,
            control_url_set: true,
            embedded: Some(EmbeddedNodeSummary {
                state: "running".into(),
                hostname: "rs-ios-k7m2q4xa".into(),
                ips: vec!["100.64.0.21".into(), "fd7a:115c:a1e0::15".into()],
                derp_region: "sha".into(),
                derp_region_name: "上海".into(),
            }),
        }),
    };
    let value = serde_json::to_value(&status).unwrap();
    assert_eq!(
        value["tailnet"],
        json!({
            "mode": "embedded",
            "active": "embedded",
            "available": true,
            "controlUrlSet": true,
            "embedded": {
                "state": "running",
                "hostname": "rs-ios-k7m2q4xa",
                "ips": ["100.64.0.21", "fd7a:115c:a1e0::15"],
                "derpRegion": "sha",
                "derpRegionName": "上海"
            }
        })
    );
    assert_eq!(
        serde_json::from_value::<NetworkStatus>(value).unwrap(),
        status
    );

    let route = status.tailnet.as_mut().unwrap();
    route.active = TailnetRoute::System;
    route.embedded = None;
    let value = serde_json::to_value(&status).unwrap();
    assert_eq!(value["tailnet"]["active"], "system");
    assert!(value["tailnet"]
        .as_object()
        .unwrap()
        .contains_key("embedded"));
    assert_eq!(value["tailnet"]["embedded"], Value::Null);
    assert_eq!(
        serde_json::from_value::<NetworkStatus>(value).unwrap(),
        status
    );

    let without_tailnet = NetworkStatus {
        ifaces: vec![],
        tailnet: None,
    };
    assert!(!serde_json::to_value(without_tailnet)
        .unwrap()
        .as_object()
        .unwrap()
        .contains_key("tailnet"));
    assert_eq!(
        serde_json::from_value::<NetworkStatus>(json!({"ifaces": []}))
            .unwrap()
            .tailnet,
        None
    );
}

#[test]
fn core_events_serialize_exactly_and_report_type() {
    let samples = event_samples();
    assert_eq!(samples.len(), 15);
    for (event, expected) in samples {
        let actual = serde_json::to_value(&event).unwrap();
        assert_eq!(actual, expected, "event {}", event.event_type());
        assert_eq!(event.event_type(), expected["type"].as_str().unwrap());
        assert_eq!(
            serde_json::from_value::<CoreEvent>(actual).unwrap(),
            event,
            "round trip {}",
            event.event_type()
        );
    }
}

#[test]
fn pair_outcome_uses_internal_camel_case_tags() {
    assert_eq!(
        serde_json::to_value(PairOutcome::Ok {
            host_id: "h".into(),
            host_name: "Mac".into(),
            permission: None,
            path: None,
            latency_ms: None,
        })
        .unwrap(),
        json!({"status": "ok", "hostId": "h", "hostName": "Mac"})
    );
    assert_eq!(
        serde_json::to_value(PairOutcome::BadCode { attempts_left: 3 }).unwrap(),
        json!({"status": "badCode", "attemptsLeft": 3})
    );
    assert_eq!(
        serde_json::to_value(PairOutcome::Mitm).unwrap(),
        json!({"status": "mitm"})
    );
    assert_eq!(
        serde_json::to_value(PairOutcome::Error {
            code: None,
            message: "boom".into(),
        })
        .unwrap(),
        json!({"status": "error", "message": "boom"})
    );
}

#[test]
fn input_dtos_ignore_unknown_fields_and_require_known_fields() {
    let cfg: CoreConfig = serde_json::from_value(json!({
        "dataDir": "/d",
        "hubUrl": "https://hs.example.com",
        "appVersion": "0.1.0",
        "platform": "ipados",
        "future": true
    }))
    .unwrap();
    assert_eq!(
        cfg,
        CoreConfig {
            files_dir: None,
            data_dir: "/d".into(),
            app_version: "0.1.0".into(),
            platform: DevicePlatform::Ipados,
        }
    );
    assert!(serde_json::from_value::<CoreConfig>(json!({
        "dataDir": "/d",
        "hubUrl": "https://hs.example.com",
        "appVersion": "0.1.0",
        "platform": "web"
    }))
    .is_err());

    let target: PairStartArg = serde_json::from_value(json!({
        "v": 1,
        "hostId": "h",
        "fp": "SHA256:fp",
        "code": "123456",
        "name": "Mac",
        "addrs": ["100.64.0.7:7400"],
        "future": true
    }))
    .unwrap();
    assert_eq!(
        target,
        PairStartArg::Target(PairTarget {
            v: 1,
            host_id: "h".into(),
            fp: "SHA256:fp".into(),
            code: "123456".into(),
            name: "Mac".into(),
            addrs: vec!["100.64.0.7:7400".into()],
        })
    );

    let code: PairStartArg = serde_json::from_value(json!({
        "addr": "studio-mac",
        "code": "123456",
        "future": true
    }))
    .unwrap();
    assert_eq!(
        code,
        PairStartArg::CodeAddr {
            addr: "studio-mac".into(),
            port: None,
            code: "123456".into(),
        }
    );
    assert_eq!(
        serde_json::to_value(PairStartArg::CodeAddr {
            addr: "studio-mac".into(),
            port: None,
            code: "123456".into(),
        })
        .unwrap(),
        json!({"addr": "studio-mac", "code": "123456"})
    );
    assert!(serde_json::from_value::<PairStartArg>(json!({})).is_err());
    assert!(serde_json::from_value::<PairStartArg>(json!({
        "v": 2,
        "hostId": "h",
        "fp": "SHA256:fp",
        "code": "123456",
        "name": "Mac",
        "addrs": []
    }))
    .is_err());

    let opts: OpenOpts = serde_json::from_value(json!({
        "intent": "view",
        "future": true
    }))
    .unwrap();
    assert_eq!(
        opts,
        OpenOpts {
            intent: OpenIntent::View,
            display_id: None,
            quality: None,
            mode: None,
            audio: None,
            max_fps: None,
            max_width: None,
            codec: None,
        }
    );

    let ssh: SshOpenOpts = serde_json::from_value(json!({
        "target": {
            "host": "10.30.0.5",
            "port": 22,
            "user": "root",
            "jump": null,
            "auth": "password"
        }
    }))
    .unwrap();
    assert_eq!(
        ssh,
        SshOpenOpts {
            tmux: None,
            target: Some(SshTarget {
                host: "10.30.0.5".into(),
                port: 22,
                user: "root".into(),
                jump: None,
                auth: SshAuth::Password,
            }),
        }
    );
}

#[test]
fn enum_strings_match_types_ts() {
    assert_eq!(serde_json::to_value(SshKeyKind::SeP256).unwrap(), "se-p256");
    assert_eq!(
        serde_json::to_value(SshKeyKind::Ed25519).unwrap(),
        "ed25519"
    );
    assert_eq!(
        serde_json::to_value(SessionActionKind::CtrlAltDel).unwrap(),
        "ctrlAltDel"
    );
    assert_eq!(
        serde_json::to_value(SessionCombo::WinKey).unwrap(),
        "winKey"
    );
    assert_eq!(
        serde_json::to_value(GoodbyeReason::HostClosedByLocalUser).unwrap(),
        "hostClosedByLocalUser"
    );
    assert_eq!(serde_json::to_value(Codec::H264).unwrap(), "h264");
    assert_eq!(
        serde_json::to_value(DevicePlatform::Macos).unwrap(),
        "macos"
    );
    assert_eq!(
        serde_json::from_value::<DevicePlatform>(json!("windows")).unwrap(),
        DevicePlatform::Windows
    );
}

#[test]
fn protobuf_and_dial_conversions_are_pure() {
    assert_eq!(
        net_path_from_pb(pb::PathKind::TailnetDerp),
        Some(NetPath::TailnetDerp)
    );
    assert_eq!(net_path_from_pb(pb::PathKind::PathUnspecified), None);
    assert_eq!(
        net_path_from_dial(&DialPath::LastGood(Box::new(DialPath::Lan))),
        NetPath::Lan
    );
    assert_eq!(
        net_path_from_dial(&DialPath::Tailnet),
        NetPath::TailnetDirect
    );

    assert_eq!(transport_from_pb(pb::Transport::Unspecified), None);
    assert_eq!(transport_from_pb(pb::Transport::Tcp), Some(Transport::Tcp));
    assert_eq!(role_from_pb(pb::Role::Unspecified), None);
    assert_eq!(role_from_pb(pb::Role::Controller), Some(Role::Controller));

    for quality in [Quality::Auto, Quality::Clear, Quality::Smooth, Quality::Eco] {
        assert_eq!(quality_from_pb(quality_to_pb(quality)), quality);
    }

    assert_eq!(
        action_kind_to_pb(SessionActionKind::Lock),
        pb::session_action::Kind::LockScreen
    );
    assert_eq!(
        action_kind_to_pb(SessionActionKind::CtrlAltDel),
        pb::session_action::Kind::CtrlAltDel
    );
    assert_eq!(
        action_kind_to_pb(SessionActionKind::LockOnEnd),
        pb::session_action::Kind::LockOnEnd
    );

    let states = [
        (
            rsp_client::SessionStateKind::Connecting,
            SessionPhase::Connecting,
        ),
        (
            rsp_client::SessionStateKind::Authenticating,
            SessionPhase::Authenticating,
        ),
        (
            rsp_client::SessionStateKind::PendingApproval,
            SessionPhase::PendingApproval,
        ),
        (
            rsp_client::SessionStateKind::Streaming,
            SessionPhase::Streaming,
        ),
        (
            rsp_client::SessionStateKind::Reconnecting,
            SessionPhase::Reconnecting,
        ),
        (rsp_client::SessionStateKind::Ended, SessionPhase::Ended),
    ];
    for (state, phase) in states {
        assert_eq!(phase_from_state(state), phase);
    }

    assert_eq!(codec_from_pb(pb::Codec::Unspecified), None);
    assert_eq!(codec_from_pb(pb::Codec::Hevc), Some(Codec::Hevc));
    assert_eq!(codec_from_pb(pb::Codec::H264), Some(Codec::H264));
}

#[test]
fn end_reasons_map_to_error_codes_and_goodbye_reasons() {
    let goodbye = pb::goodbye::Reason::Replaced;
    assert_eq!(
        end_reason_to_error(&EndReason::Goodbye(goodbye)),
        (
            rsp_client::goodbye_reason_error(goodbye).map(|code| u32::from(code.code())),
            Some(GoodbyeReason::Replaced)
        )
    );
    assert_eq!(
        end_reason_to_error(&EndReason::Protocol),
        (None, Some(GoodbyeReason::ProtocolError))
    );
    assert_eq!(
        end_reason_to_error(&EndReason::ConnectionLost),
        (None, None)
    );
    assert_eq!(
        end_reason_to_error(&EndReason::Error(
            rsp_core::errors::ErrorCode::ApprovalDenied
        )),
        (Some(140), None)
    );
    assert_eq!(end_reason_to_error(&EndReason::Close(3)).0, None);
}

#[test]
fn participant_and_display_convert_from_protobuf() {
    let participant = Participant::from(&pb::Participant {
        device_id: "d".into(),
        device_name: "iPad".into(),
        user: "me".into(),
        path: pb::PathKind::Lan as i32,
        since_us: 5,
    });
    assert_eq!(participant.path, Some(NetPath::Lan));
    assert_eq!(participant.since, 5);

    let without_refresh = SessionDisplay::from(&pb::Display {
        id: 1,
        name: "Built-in".into(),
        width_px: 2_560,
        height_px: 1_600,
        primary: true,
        refresh_hz: 0,
        ..pb::Display::default()
    });
    assert_eq!(without_refresh.refresh_hz, None);
    assert_eq!(
        serde_json::to_value(&without_refresh).unwrap(),
        json!({
            "id": 1,
            "name": "Built-in",
            "widthPx": 2560,
            "heightPx": 1600,
            "primary": true
        })
    );

    let with_refresh = SessionDisplay::from(&pb::Display {
        refresh_hz: 120,
        ..pb::Display::default()
    });
    assert_eq!(with_refresh.refresh_hz, Some(120));
}

#[test]
fn ids_are_decimal_strings() {
    assert_eq!(id_str(18446744073709551615), "18446744073709551615");
    assert_eq!(parse_id("18446744073709551615"), Some(u64::MAX));
    assert_eq!(parse_id("17"), Some(17));
    assert_eq!(parse_id("x"), None);
    assert_eq!(parse_id("-1"), None);
}

#[test]
fn virtual_display_json_preserves_old_displays_and_uses_camel_case_statuses() {
    let old =
        json!({"id": 1, "name": "Built-in", "widthPx": 2560, "heightPx": 1600, "primary": true});
    let physical: SessionDisplay = serde_json::from_value(old.clone()).unwrap();
    assert!(!physical.is_virtual);
    assert_eq!(serde_json::to_value(&physical).unwrap(), old);

    let display = SessionDisplay::from(&pb::Display {
        id: 3,
        name: "Virtual".into(),
        width_px: 2732,
        height_px: 2048,
        refresh_hz: 60,
        is_virtual: true,
        ..Default::default()
    });
    for (status, name) in [
        (VdisplayStatus::Ok, "ok"),
        (VdisplayStatus::Unsupported, "unsupported"),
        (VdisplayStatus::NotAllowed, "notAllowed"),
        (VdisplayStatus::Limit, "limit"),
        (VdisplayStatus::BadRequest, "badRequest"),
        (VdisplayStatus::Failed, "failed"),
    ] {
        let event = CoreEvent::SessionVdisplay {
            session_id: "7".into(),
            request_id: 1,
            status,
            display: (status == VdisplayStatus::Ok).then_some(display.clone()),
            message: None,
        };
        assert_eq!(event.event_type(), "session.vdisplay");
        let mut expected = json!({
            "type": "session.vdisplay", "sessionId": "7", "requestId": 1, "status": name
        });
        if status == VdisplayStatus::Ok {
            expected["display"] = json!({
                "id": 3, "name": "Virtual", "widthPx": 2732, "heightPx": 2048,
                "primary": false, "refreshHz": 60, "isVirtual": true
            });
        }
        let actual = serde_json::to_value(&event).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(serde_json::from_value::<CoreEvent>(actual).unwrap(), event);
    }
}
