use rs_mobile::rscore::{
    dto_core::{NetPath, Role, SessionPhase, SessionSummary, Transport},
    sessmap::{MapperSnapshot, SessionMapper},
};
use rsp_client::{EndReason, SessionEvent, SessionStateKind, SessionStats};
use rsp_core::{errors::ErrorCode, pb, stats::ClientStats};
use serde_json::{json, Value};

pub fn map_json(mapper: &mut SessionMapper, event: SessionEvent) -> Value {
    serde_json::to_value(mapper.map(&event)).unwrap()
}

pub fn displays() -> Vec<pb::Display> {
    vec![
        pb::Display {
            id: 1,
            name: "Built-in".into(),
            width_px: 2560,
            height_px: 1600,
            ..Default::default()
        },
        pb::Display {
            id: 2,
            name: "External".into(),
            width_px: 1920,
            height_px: 1080,
            primary: true,
            refresh_hz: 60,
            ..Default::default()
        },
    ]
}

pub fn displays_json() -> Value {
    json!([
        {"id": 1, "name": "Built-in", "widthPx": 2560, "heightPx": 1600, "primary": false},
        {"id": 2, "name": "External", "widthPx": 1920, "heightPx": 1080, "primary": true, "refreshHz": 60}
    ])
}

pub fn stream() -> pb::StreamConfig {
    pb::StreamConfig {
        display_id: 2,
        width: 1920,
        height: 1080,
        codec: pb::Codec::Hevc as i32,
        ..Default::default()
    }
}

pub fn hud() -> ClientStats {
    ClientStats {
        fps: 60,
        recv_kbps: 8_000,
        loss_pct: 1.25,
        frames_lost: 0,
        fec_recovered: 0,
        decode_p50_us: 4_560,
        decode_p95_us: 5_000,
        latency_p50_us: 20_000,
        latency_p95_us: 30_000,
        rtt_us: 12_345,
        path: pb::PathKind::TailnetDerp,
        transport: pb::Transport::Tcp,
    }
}

pub fn participant() -> pb::Participant {
    pb::Participant {
        device_id: "device-1".into(),
        device_name: "iPad".into(),
        user: "user@example.com".into(),
        path: pb::PathKind::Lan as i32,
        since_us: 123,
    }
}

pub fn participant_json() -> Value {
    json!({"deviceId": "device-1", "deviceName": "iPad", "user": "user@example.com", "path": "lan", "since": 123})
}

pub fn received_frame() -> rsp_client::ReceivedFrame {
    rsp_client::ReceivedFrame {
        frame: rsp_core::video::Frame {
            frame_id: 1,
            epoch: 1,
            display_id: 2,
            flags: 0,
            keyframe: true,
            capture_ts_us: 123,
            data: vec![1, 2, 3],
        },
        received_us: 456,
        capture_local_us: None,
    }
}

#[test]
pub fn host_clipboard_maps_to_from_host_event_without_logging_payload() {
    let mut mapper = SessionMapper::new(7, "host-1");
    let item = rsp_core::clipboard::ClipboardItem::new(
        rsp_core::clipboard::ClipboardKind::Text,
        b"RS-CLIP-1".to_vec(),
    )
    .unwrap();
    let events = mapper.map(&SessionEvent::Clipboard(item));
    assert_eq!(events.len(), 1);
    let value = serde_json::to_value(&events[0]).unwrap();
    assert_eq!(value["type"], "session.clipboard");
    assert_eq!(value["sessionId"], "7");
    assert_eq!(value["direction"], "fromHost");
    assert_eq!(value["text"], "RS-CLIP-1");
    assert!(value["at"].as_u64().is_some());
}

#[test]
pub fn snapshot_defaults_to_connecting() {
    let snapshot = MapperSnapshot::default();
    assert_eq!(snapshot.phase, SessionPhase::Connecting);
    assert!(snapshot.role.is_none());
    assert!(snapshot.path.is_none());
    assert!(snapshot.transport.is_none());
    assert!(snapshot.derp_region.is_none());
    assert!(snapshot.host_name.is_none());
    assert!(snapshot.displays.is_empty());
    assert!(snapshot.current_display.is_none());
    assert!(snapshot.stream.is_none());
    assert!(snapshot.controller.is_none());
    assert!(snapshot.viewers.is_empty());
    assert!(!snapshot.input_paused_local);
    assert!(snapshot.rate_reason.is_none());
    assert!(!snapshot.ended);
}

#[test]
pub fn normal_session_serializes_the_expected_event_sequence() {
    let mut mapper = SessionMapper::new(7, "host-1");
    let mut output = Vec::new();
    for event in [
        SessionEvent::StateChanged(SessionStateKind::Connecting),
        SessionEvent::StateChanged(SessionStateKind::Authenticating),
        SessionEvent::ServerHello(pb::ServerHello {
            host_name: "Mac".into(),
            peer: Some(pb::PeerInfo {
                path: pb::PathKind::Lan as i32,
                ..Default::default()
            }),
            ..Default::default()
        }),
        SessionEvent::SessionState(pb::SessionState {
            your_role: pb::Role::Controller as i32,
            ..Default::default()
        }),
        SessionEvent::Displays(displays()),
        SessionEvent::StreamConfig(stream()),
        SessionEvent::StateChanged(SessionStateKind::Streaming),
    ] {
        output.extend(mapper.map(&event));
    }
    assert_eq!(
        serde_json::to_value(output).unwrap(),
        json!([
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "authenticating"},
            {"type": "session.role", "sessionId": "7", "role": "controller", "controller": null, "viewers": [], "inputPausedLocal": false},
            {"type": "session.displays", "sessionId": "7", "displays": displays_json(), "currentId": 2},
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "streaming", "path": "lan"}
        ])
    );
    assert_eq!(mapper.snapshot().host_name.as_deref(), Some("Mac"));
    assert_eq!(mapper.snapshot().stream, Some(stream()));
    assert_eq!(mapper.snapshot().current_display, Some(2));
}

#[test]
pub fn approval_reconnect_and_resume_use_documented_defaults() {
    let mut mapper = SessionMapper::new(7, "host-1");
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::StateChanged(SessionStateKind::PendingApproval)
        ),
        json!([
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "pendingApproval", "approvalTimeoutS": 30}
        ])
    );
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::StateChanged(SessionStateKind::Reconnecting)
        ),
        json!([
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "reconnecting", "reconnectReason": "pathLost"}
        ])
    );
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::StateChanged(SessionStateKind::Streaming)
        ),
        json!([
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "streaming", "resumed": true}
        ])
    );
}

#[test]
pub fn repeated_phase_and_role_events_are_suppressed() {
    let mut mapper = SessionMapper::new(7, "host-1");
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::StateChanged(SessionStateKind::Streaming)
        ),
        json!([
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "streaming"}
        ])
    );
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::StateChanged(SessionStateKind::Streaming)
        ),
        json!([])
    );
    assert_eq!(
        map_json(&mut mapper, SessionEvent::Role(pb::Role::Viewer)),
        json!([
            {"type": "session.role", "sessionId": "7", "role": "viewer", "controller": null, "viewers": [], "inputPausedLocal": false}
        ])
    );
    assert_eq!(
        map_json(&mut mapper, SessionEvent::Role(pb::Role::Viewer)),
        json!([])
    );
    assert_eq!(
        map_json(&mut mapper, SessionEvent::Role(pb::Role::Unspecified)),
        json!([])
    );
    assert!(mapper.snapshot().role.is_none());
}

#[test]
pub fn role_and_local_activity_reuse_the_latest_roster() {
    let mut mapper = SessionMapper::new(7, "host-1");
    assert_eq!(
        map_json(&mut mapper, SessionEvent::LocalUse(true)),
        json!([])
    );
    let state = pb::SessionState {
        your_role: pb::Role::Viewer as i32,
        controller: Some(participant()),
        viewers: vec![participant()],
        input_paused_local: true,
    };
    let expected = json!([{
        "type": "session.role", "sessionId": "7", "role": "viewer", "controller": participant_json(),
        "viewers": [participant_json()], "inputPausedLocal": true
    }]);
    assert_eq!(
        map_json(&mut mapper, SessionEvent::SessionState(state.clone())),
        expected
    );
    // A SessionState always reports the roster even if the role did not change.
    assert_eq!(
        map_json(&mut mapper, SessionEvent::SessionState(state)),
        expected
    );
    assert_eq!(
        map_json(&mut mapper, SessionEvent::LocalUse(true)),
        json!([])
    );
    assert_eq!(
        map_json(&mut mapper, SessionEvent::LocalUse(false)),
        json!([{
            "type": "session.role", "sessionId": "7", "role": "viewer", "controller": participant_json(),
            "viewers": [participant_json()], "inputPausedLocal": false
        }])
    );
    assert_eq!(
        map_json(&mut mapper, SessionEvent::Role(pb::Role::Controller)),
        json!([{
            "type": "session.role", "sessionId": "7", "role": "controller", "controller": participant_json(),
            "viewers": [participant_json()], "inputPausedLocal": false
        }])
    );
    assert_eq!(
        mapper.snapshot().controller.as_ref().unwrap().device_id,
        "device-1"
    );
    assert_eq!(mapper.snapshot().viewers.len(), 1);
    assert!(!mapper.snapshot().input_paused_local);
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::SessionState(pb::SessionState::default())
        ),
        json!([])
    );
    assert!(mapper.snapshot().controller.is_none());
    assert!(mapper.snapshot().viewers.is_empty());
}

#[test]
pub fn server_hello_bootstraps_displays_without_replacing_an_existing_list() {
    let mut mapper = SessionMapper::new(7, "host-1");
    let hello = pb::ServerHello {
        host_name: "Mac".into(),
        displays: displays(),
        ..Default::default()
    };
    assert_eq!(
        map_json(&mut mapper, SessionEvent::ServerHello(hello.clone())),
        json!([
            {"type": "session.displays", "sessionId": "7", "displays": displays_json(), "currentId": 2}
        ])
    );
    let mut changed_hello = hello;
    changed_hello.displays[0].name = "Ignored".into();
    assert_eq!(
        map_json(&mut mapper, SessionEvent::ServerHello(changed_hello)),
        json!([])
    );
    assert_eq!(mapper.snapshot().displays[0].name, "Built-in");
    let changed_stream = pb::StreamConfig {
        display_id: 1,
        ..stream()
    };
    assert_eq!(
        map_json(&mut mapper, SessionEvent::StreamConfig(changed_stream)),
        json!([
            {"type": "session.displays", "sessionId": "7", "displays": displays_json(), "currentId": 1}
        ])
    );
    assert_eq!(
        map_json(&mut mapper, SessionEvent::StreamConfig(changed_stream)),
        json!([])
    );
    assert_eq!(
        map_json(&mut mapper, SessionEvent::Displays(displays())),
        json!([
            {"type": "session.displays", "sessionId": "7", "displays": displays_json(), "currentId": 1}
        ])
    );
}

#[test]
pub fn display_selection_falls_back_to_first_or_zero_and_respects_stream_config() {
    let mut mapper = SessionMapper::new(7, "host-1");
    assert_eq!(
        map_json(&mut mapper, SessionEvent::Displays(vec![])),
        json!([
            {"type": "session.displays", "sessionId": "7", "displays": [], "currentId": 0}
        ])
    );
    assert!(mapper.snapshot().current_display.is_none());
    let mut no_primary = displays();
    no_primary[1].primary = false;
    let value = map_json(&mut mapper, SessionEvent::Displays(no_primary));
    assert_eq!(value[0]["currentId"], 1);

    let mut mapper = SessionMapper::new(7, "host-1");
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::StreamConfig(pb::StreamConfig {
                display_id: 9,
                ..stream()
            })
        ),
        json!([])
    );
    let value = map_json(&mut mapper, SessionEvent::Displays(displays()));
    assert_eq!(value[0]["currentId"], 9);
    assert_eq!(mapper.snapshot().current_display, Some(9));
    let value = map_json(&mut mapper, SessionEvent::Displays(vec![]));
    assert_eq!(value[0]["currentId"], 9);
}

#[test]
pub fn path_sources_preserve_known_values_when_fields_are_unspecified() {
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.set_dial_result(NetPath::Lan, Transport::Quic);
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::ServerHello(pb::ServerHello {
                peer: Some(pb::PeerInfo {
                    path: pb::PathKind::TailnetDerp as i32,
                    derp_region: "example-region".into(),
                    ..Default::default()
                }),
                ..Default::default()
            })
        ),
        json!([])
    );
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::HostStats(pb::HostStats::default())
        ),
        json!([])
    );
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::ServerHello(pb::ServerHello {
                peer: Some(pb::PeerInfo {
                    path: 999,
                    ..Default::default()
                }),
                ..Default::default()
            })
        ),
        json!([])
    );
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::StateChanged(SessionStateKind::Streaming)
        ),
        json!([
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "streaming", "path": "tailnetDerp", "transport": "quic", "derpRegion": "example-region"}
        ])
    );
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::HostStats(pb::HostStats {
                path: pb::PathKind::Intranet as i32,
                derp_region: "updated-region".into(),
                ..Default::default()
            })
        ),
        json!([])
    );
    assert_eq!(mapper.snapshot().path, Some(NetPath::Intranet));
    assert_eq!(
        mapper.snapshot().derp_region.as_deref(),
        Some("updated-region")
    );
}

#[test]
pub fn stats_require_hud_stream_and_known_path_and_transport() {
    let mut mapper = SessionMapper::new(7, "host-1");
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::Stats(SessionStats {
                hud: Some(hud()),
                ..Default::default()
            })
        ),
        json!([])
    );
    // Even a HUD received before StreamConfig enriches the snapshot.
    assert_eq!(mapper.snapshot().path, Some(NetPath::TailnetDerp));
    assert_eq!(mapper.snapshot().transport, Some(Transport::Tcp));
    assert_eq!(
        map_json(&mut mapper, SessionEvent::StreamConfig(stream())),
        json!([])
    );
    assert_eq!(
        map_json(&mut mapper, SessionEvent::Stats(SessionStats::default())),
        json!([])
    );

    for (path, transport) in [
        (pb::PathKind::PathUnspecified, pb::Transport::Tcp),
        (pb::PathKind::Lan, pb::Transport::Unspecified),
        (pb::PathKind::PathUnspecified, pb::Transport::Unspecified),
    ] {
        let mut unknown = SessionMapper::new(7, "host-1");
        unknown.map(&SessionEvent::StreamConfig(stream()));
        assert_eq!(
            map_json(
                &mut unknown,
                SessionEvent::Stats(SessionStats {
                    hud: Some(ClientStats {
                        path,
                        transport,
                        ..hud()
                    }),
                    ..Default::default()
                })
            ),
            json!([])
        );
    }
}

#[test]
pub fn stats_round_decode_and_loss_with_the_current_integer_rtt_contract() {
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.map(&SessionEvent::StreamConfig(stream()));
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::Stats(SessionStats {
                rtt_us: Some(99_000),
                hud: Some(hud()),
                ..Default::default()
            })
        ),
        json!([{
            "type": "session.stats", "sessionId": "7",
            // Contract gap: dto_core.rtt_ms is u32; f64 is required to emit 12.3.
            "rttMs": 12, "fps": 60, "bitrateKbps": 8000, "decodeMs": 4.6, "lossPct": 1.3,
            "width": 1920, "height": 1080, "codec": "hevc", "path": "tailnetDerp", "transport": "tcp"
        }])
    );
    assert_eq!(mapper.snapshot().path, Some(NetPath::TailnetDerp));
    assert_eq!(mapper.snapshot().transport, Some(Transport::Tcp));
}

#[test]
pub fn stats_fall_back_to_snapshot_and_session_rtt_and_keep_rate_reason() {
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.set_dial_result(NetPath::Manual, Transport::Quic);
    mapper.map(&SessionEvent::StreamConfig(pb::StreamConfig {
        codec: pb::Codec::H264 as i32,
        ..stream()
    }));
    mapper.map(&SessionEvent::HostStats(pb::HostStats {
        derp_region: "example-region".into(),
        ..Default::default()
    }));
    mapper.map(&SessionEvent::RateUpdate(pb::RateUpdate {
        reason: "loss".into(),
        ..Default::default()
    }));
    mapper.map(&SessionEvent::RateUpdate(pb::RateUpdate::default()));
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::Stats(SessionStats {
                rtt_us: Some(17_000),
                hud: Some(ClientStats {
                    rtt_us: 0,
                    path: pb::PathKind::PathUnspecified,
                    transport: pb::Transport::Unspecified,
                    ..hud()
                }),
                ..Default::default()
            })
        ),
        json!([{
            "type": "session.stats", "sessionId": "7", "rttMs": 17, "fps": 60, "bitrateKbps": 8000,
            "decodeMs": 4.6, "lossPct": 1.3, "width": 1920, "height": 1080, "codec": "h264",
            "path": "manual", "transport": "quic", "derpRegion": "example-region", "rateReason": "loss"
        }])
    );
    assert_eq!(mapper.snapshot().rate_reason.as_deref(), Some("loss"));
    let value = map_json(
        &mut mapper,
        SessionEvent::Stats(SessionStats {
            hud: Some(ClientStats { rtt_us: 0, ..hud() }),
            ..Default::default()
        }),
    );
    assert_eq!(value[0]["rttMs"], 0);
    let value = map_json(
        &mut mapper,
        SessionEvent::Stats(SessionStats {
            rtt_us: Some(u64::MAX),
            hud: Some(ClientStats { rtt_us: 0, ..hud() }),
            ..Default::default()
        }),
    );
    assert_eq!(value[0]["rttMs"], u32::MAX);
}

#[test]
pub fn control_requests_and_idle_warning_serialize_the_expected_fields() {
    let mut mapper = SessionMapper::new(7, "host-1");
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::ControlRequested(pb::ControlRequest {
                requester: Some(participant()),
                timeout_s: 30,
            })
        ),
        json!([
            {"type": "session.controlRequest", "sessionId": "7", "requester": participant_json(), "timeoutS": 30}
        ])
    );
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::ControlRequested(pb::ControlRequest {
                requester: None,
                timeout_s: 5
            })
        ),
        json!([
            {"type": "session.controlRequest", "sessionId": "7", "requester": {"deviceId": "", "deviceName": "", "user": "", "path": null, "since": 0}, "timeoutS": 5}
        ])
    );
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::IdleWarning(pb::IdleWarning {
                seconds_left: 60,
                lock_on_end: true
            })
        ),
        json!([
            {"type": "session.idleWarning", "sessionId": "7", "secondsLeft": 60, "lockOnEnd": true}
        ])
    );
}

#[test]
pub fn duplicate_errors_are_suppressed_and_replaced_ends_the_session() {
    let mut mapper = SessionMapper::new(7, "host-1");
    let event = SessionEvent::Error {
        code: ErrorCode::Other,
        message: "x".into(),
    };
    assert_eq!(
        serde_json::to_value(mapper.map(&event)).unwrap(),
        json!([
            {"type": "session.error", "sessionId": "7", "hostId": "host-1", "code": 199, "message": "x"}
        ])
    );
    assert!(mapper.map(&event).is_empty());
    let reason = pb::goodbye::Reason::Replaced;
    let code = rsp_client::goodbye_reason_error(reason).map(|error| u64::from(error.code()));
    let output = map_json(
        &mut mapper,
        SessionEvent::Ended {
            code: None,
            reason: "replaced by new session".into(),
            kind: EndReason::Goodbye(reason),
        },
    );
    assert_eq!(
        output,
        json!([
            {"type": "session.error", "sessionId": "7", "hostId": "host-1", "reason": "replaced", "message": "replaced by new session"},
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "ended"}
        ])
    );
    assert_eq!(output[0].get("code").and_then(Value::as_u64), code);
    assert!(mapper.snapshot().ended);
    assert_eq!(mapper.snapshot().phase, SessionPhase::Ended);
    let frozen = format!("{:?}", mapper.snapshot());
    for event in [
        event,
        SessionEvent::StateChanged(SessionStateKind::Streaming),
        SessionEvent::SessionState(pb::SessionState::default()),
        SessionEvent::Role(pb::Role::Viewer),
        SessionEvent::Displays(displays()),
        SessionEvent::StreamConfig(stream()),
        SessionEvent::LocalUse(true),
        SessionEvent::HostStats(pb::HostStats::default()),
        SessionEvent::RateUpdate(pb::RateUpdate::default()),
        SessionEvent::ControlRequested(pb::ControlRequest::default()),
        SessionEvent::IdleWarning(pb::IdleWarning::default()),
        SessionEvent::PairResult(pb::PairResult::default()),
        SessionEvent::Frame(received_frame()),
        SessionEvent::Cursor(rsp_client::CursorEvent {
            x: 0.5,
            y: 0.5,
            visible: true,
        }),
        SessionEvent::CursorMessage(pb::CursorMsg::default()),
        SessionEvent::ClockSync {
            rtt_us: 12_345,
            offset_us: -100,
        },
        SessionEvent::ServerHello(pb::ServerHello {
            host_name: "Ignored".into(),
            ..Default::default()
        }),
        SessionEvent::Stats(SessionStats {
            hud: Some(hud()),
            ..Default::default()
        }),
        SessionEvent::Ended {
            code: None,
            reason: "again".into(),
            kind: EndReason::ConnectionLost,
        },
    ] {
        assert!(mapper.map(&event).is_empty());
    }
    mapper.set_dial_result(NetPath::Lan, Transport::Tcp);
    assert_eq!(format!("{:?}", mapper.snapshot()), frozen);
}

#[test]
pub fn connection_lost_has_no_invented_code_or_reason() {
    let mut mapper = SessionMapper::new(7, "host-1");
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::Ended {
                code: None,
                reason: "ConnectionLost".into(),
                kind: EndReason::ConnectionLost,
            }
        ),
        json!([
            {"type": "session.error", "sessionId": "7", "hostId": "host-1", "message": "ConnectionLost"},
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "ended"}
        ])
    );
}

#[test]
pub fn terminal_code_overrides_fallback_and_duplicate_code_is_not_reemitted() {
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.map(&SessionEvent::Error {
        code: ErrorCode::ApprovalDenied,
        message: "x".into(),
    });
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::Error {
                code: ErrorCode::ApprovalDenied,
                message: "changed".into()
            }
        )[0]["message"],
        "changed"
    );
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::Ended {
                code: Some(ErrorCode::ApprovalDenied),
                reason: "closed".into(),
                kind: EndReason::Error(ErrorCode::Other),
            }
        ),
        json!([
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "ended"}
        ])
    );

    let mut mapper = SessionMapper::new(7, "host-1");
    let value = map_json(
        &mut mapper,
        SessionEvent::Ended {
            code: Some(ErrorCode::ApprovalDenied),
            reason: "closed".into(),
            kind: EndReason::Error(ErrorCode::Other),
        },
    );
    assert_eq!(
        value[0],
        json!({"type": "session.error", "sessionId": "7", "hostId": "host-1", "code": 140, "message": "closed"})
    );
}

#[test]
pub fn terminal_reason_survives_an_earlier_ended_state() {
    let mut mapper = SessionMapper::new(7, "host-1");
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::StateChanged(SessionStateKind::Ended)
        ),
        json!([
            {"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "ended"}
        ])
    );
    assert!(!mapper.snapshot().ended);
    assert_eq!(
        map_json(
            &mut mapper,
            SessionEvent::Ended {
                code: None,
                reason: "bad protocol".into(),
                kind: EndReason::Protocol
            }
        ),
        json!([
            {"type": "session.error", "sessionId": "7", "hostId": "host-1", "reason": "protocolError", "message": "bad protocol"}
        ])
    );
    assert!(mapper.snapshot().ended);
}

#[test]
pub fn goodbye_reason_is_emitted_even_when_its_code_matches_the_last_error() {
    let reason = pb::goodbye::Reason::Revoked;
    let code = rsp_client::goodbye_reason_error(reason).unwrap();
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.map(&SessionEvent::Error {
        code,
        message: "x".into(),
    });
    let value = map_json(
        &mut mapper,
        SessionEvent::Ended {
            code: None,
            reason: "revoked".into(),
            kind: EndReason::Goodbye(reason),
        },
    );
    assert_eq!(value[0]["type"], "session.error");
    assert_eq!(value[0]["reason"], "revoked");
    assert_eq!(value[0]["code"], 180);
    assert_eq!(value[1]["phase"], "ended");
}

#[test]
pub fn summary_applies_snapshot_fields_and_keeps_existing_name_when_hello_is_empty() {
    let mut mapper = SessionMapper::new(u64::MAX, "host-1");
    let mut summary = SessionSummary {
        id: u64::MAX.to_string(),
        host_id: "host-1".into(),
        host_name: "Existing".into(),
        phase: SessionPhase::Ended,
        role: Some(Role::Viewer),
        path: Some(NetPath::Manual),
        transport: Some(Transport::Tcp),
        started_at: 123,
        display_id: Some(99),
    };
    mapper.apply_to_summary(&mut summary);
    assert_eq!(summary.phase, SessionPhase::Connecting);
    assert!(summary.role.is_none());
    assert!(summary.path.is_none());
    assert!(summary.transport.is_none());
    assert!(summary.display_id.is_none());
    assert_eq!(summary.host_name, "Existing");

    mapper.set_dial_result(NetPath::Lan, Transport::Quic);
    mapper.map(&SessionEvent::ServerHello(pb::ServerHello {
        host_name: "Mac".into(),
        ..Default::default()
    }));
    mapper.map(&SessionEvent::Role(pb::Role::Controller));
    mapper.map(&SessionEvent::StreamConfig(stream()));
    let value = map_json(
        &mut mapper,
        SessionEvent::StateChanged(SessionStateKind::Streaming),
    );
    assert_eq!(value[0]["sessionId"], u64::MAX.to_string());
    mapper.apply_to_summary(&mut summary);
    assert_eq!(
        serde_json::to_value(&summary).unwrap(),
        json!({
            "id": u64::MAX.to_string(), "hostId": "host-1", "hostName": "Mac", "phase": "streaming",
            "role": "controller", "path": "lan", "transport": "quic", "startedAt": 123, "displayId": 2
        })
    );
    mapper.map(&SessionEvent::ServerHello(pb::ServerHello::default()));
    mapper.apply_to_summary(&mut summary);
    assert_eq!(summary.host_name, "Mac");
}

#[test]
pub fn pairing_frame_cursor_and_clock_events_do_not_emit_core_events() {
    let mut mapper = SessionMapper::new(7, "host-1");
    for event in [
        SessionEvent::PairResult(pb::PairResult::default()),
        SessionEvent::Frame(received_frame()),
        SessionEvent::Cursor(rsp_client::CursorEvent {
            x: 0.5,
            y: 0.5,
            visible: true,
        }),
        SessionEvent::CursorMessage(pb::CursorMsg::default()),
        SessionEvent::ClockSync {
            rtt_us: 12_345,
            offset_us: -100,
        },
    ] {
        assert_eq!(map_json(&mut mapper, event), json!([]));
    }
    assert_eq!(mapper.snapshot().phase, SessionPhase::Connecting);
}
#[test]
fn token_resume_hides_handshake_and_uses_auth_result_resumed() {
    for resumed in [true, false] {
        let mut mapper = SessionMapper::new(7, "host-1");
        mapper.set_dial_result(NetPath::Lan, Transport::Quic);
        mapper.map(&SessionEvent::StateChanged(SessionStateKind::Streaming));
        assert_eq!(
            serde_json::to_value(mapper.begin_resume()).unwrap(),
            json!({"type": "session.state", "sessionId": "7", "hostId": "host-1", "phase": "reconnecting", "reconnectReason": "resume", "path": "lan", "transport": "quic"})
        );
        assert_eq!(mapper.snapshot().phase, SessionPhase::Reconnecting);
        for state in [
            SessionStateKind::Connecting,
            SessionStateKind::Authenticating,
        ] {
            assert!(mapper.map(&SessionEvent::StateChanged(state)).is_empty());
        }
        mapper.set_auth_resumed(resumed);
        let events = map_json(
            &mut mapper,
            SessionEvent::StateChanged(SessionStateKind::Streaming),
        );
        assert_eq!(events[0]["phase"], "streaming");
        assert_eq!(events[0]["resumed"], resumed);
        assert!(events[0].get("reconnectReason").is_none());
        // A later ordinary transition is no longer part of token recovery.
        assert!(!mapper
            .map(&SessionEvent::StateChanged(
                SessionStateKind::Authenticating
            ))
            .is_empty());
    }
}

#[test]
fn token_resume_pending_approval_returns_to_normal_mapping() {
    let mut mapper = SessionMapper::new(7, "host-1");
    mapper.begin_resume();
    let pending = map_json(
        &mut mapper,
        SessionEvent::StateChanged(SessionStateKind::PendingApproval),
    );
    assert_eq!(pending[0]["phase"], "pendingApproval");
    assert!(pending[0].get("resumed").is_none());
    assert!(!mapper
        .map(&SessionEvent::StateChanged(
            SessionStateKind::Authenticating
        ))
        .is_empty());
}

#[test]
fn virtual_displays_keep_the_marker_in_session_display_events() {
    let mut mapper = SessionMapper::new(7, "host-1");
    let mut list = displays();
    list.push(pb::Display {
        id: 3,
        name: "Virtual".into(),
        width_px: 2732,
        height_px: 2048,
        refresh_hz: 60,
        is_virtual: true,
        ..Default::default()
    });
    let mut expected = displays_json();
    expected.as_array_mut().unwrap().push(json!({
        "id": 3, "name": "Virtual", "widthPx": 2732, "heightPx": 2048,
        "primary": false, "refreshHz": 60, "isVirtual": true
    }));
    assert_eq!(
        map_json(&mut mapper, SessionEvent::Displays(list)),
        json!([{"type": "session.displays", "sessionId": "7", "displays": expected, "currentId": 2}])
    );
    assert!(mapper.snapshot().displays[2].is_virtual);
}
