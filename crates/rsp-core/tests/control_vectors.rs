use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use prost::Message;
use rsp_core::control::{encode, EnvelopeDecoder};
use rsp_core::pb;
use serde_json::{json, Value};

const CONTROL_ONEOF_MEMBERS: usize = 44;

static VECTOR_FILES_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/control")
}

fn envelope_name(env: &pb::Envelope) -> &'static str {
    match env.msg.as_ref().expect("vector envelope has a message") {
        pb::envelope::Msg::ClientHello(_) => "client_hello",
        pb::envelope::Msg::ServerHello(_) => "server_hello",
        pb::envelope::Msg::AuthResult(_) => "auth_result",
        pb::envelope::Msg::SessionState(_) => "session_state",
        pb::envelope::Msg::StartStream(_) => "start_stream",
        pb::envelope::Msg::StreamConfig(_) => "stream_config",
        pb::envelope::Msg::KeyframeRequest(_) => "keyframe_request",
        pb::envelope::Msg::Feedback(_) => "feedback",
        pb::envelope::Msg::RateUpdate(_) => "rate_update",
        pb::envelope::Msg::Ping(_) => "ping",
        pb::envelope::Msg::Pong(_) => "pong",
        pb::envelope::Msg::DisplayList(_) => "display_list",
        pb::envelope::Msg::SelectDisplay(_) => "select_display",
        pb::envelope::Msg::RequestControl(_) => "request_control",
        pb::envelope::Msg::ControlRequest(_) => "control_request",
        pb::envelope::Msg::ControlResponse(_) => "control_response",
        pb::envelope::Msg::LocalActivity(_) => "local_activity",
        pb::envelope::Msg::IdleWarning(_) => "idle_warning",
        pb::envelope::Msg::PauseStream(_) => "pause_stream",
        pb::envelope::Msg::ResumeStream(_) => "resume_stream",
        pb::envelope::Msg::HostStats(_) => "host_stats",
        pb::envelope::Msg::SessionAction(_) => "session_action",
        pb::envelope::Msg::SetQuality(_) => "set_quality",
        pb::envelope::Msg::SetAudio(_) => "set_audio",
        pb::envelope::Msg::RecordingNotice(_) => "recording_notice",
        pb::envelope::Msg::CreateVirtualDisplay(_) => "create_virtual_display",
        pb::envelope::Msg::VirtualDisplayResult(_) => "virtual_display_result",
        pb::envelope::Msg::ReleaseVirtualDisplay(_) => "release_virtual_display",
        pb::envelope::Msg::Viewport(_) => "viewport",
        pb::envelope::Msg::ViewportAck(_) => "viewport_ack",
        pb::envelope::Msg::AttachInput(_) => "attach_input",
        pb::envelope::Msg::PairStart(_) => "pair_start",
        pb::envelope::Msg::PairSpake(_) => "pair_spake",
        pb::envelope::Msg::PairConfirm(_) => "pair_confirm",
        pb::envelope::Msg::PairResult(_) => "pair_result",
        pb::envelope::Msg::PairPassword(_) => "pair_password",
        pb::envelope::Msg::Error(_) => "error",
        pb::envelope::Msg::Goodbye(_) => "goodbye",
        pb::envelope::Msg::FileList(_) => "file_list",
        pb::envelope::Msg::FileListing(_) => "file_listing",
        pb::envelope::Msg::FileRequest(_) => "file_request",
        pb::envelope::Msg::FileQuery(_) => "file_query",
        pb::envelope::Msg::FileStatus(_) => "file_status",
        pb::envelope::Msg::FileCancel(_) => "file_cancel",
    }
}

fn display() -> pb::Display {
    pb::Display {
        id: 7,
        name: "DELL U2723QE".into(),
        width_px: 3840,
        height_px: 2160,
        scale: 1.5,
        x: -1920,
        y: 120,
        refresh_hz: 60,
        primary: true,
        rotation: 90,
        is_virtual: false,
    }
}

fn participant() -> pb::Participant {
    pb::Participant {
        device_id: "device-07".into(),
        device_name: "Alice's iPad".into(),
        user: "alice@".into(),
        path: pb::PathKind::TailnetDerp as i32,
        since_us: 1_726_000_000_123_456,
    }
}

fn vectors() -> Vec<(&'static str, pb::Envelope, Value)> {
    let client_hello = pb::Envelope::from(pb::ClientHello {
        protocol: 1,
        app_version: "0.3.1".into(),
        features: 0x0000_0000_0000_2007,
        required_features: 0x0000_0000_0000_0001,
        device_name: "Alice's iPhone".into(),
        platform: pb::Platform::Ios as i32,
        intent: pb::Intent::Pair as i32,
        resume_token: vec![0x11; 16],
        caps: Some(pb::ClientCaps {
            decode: vec![pb::Codec::Hevc as i32, pb::Codec::H264 as i32],
            max_width: 2732,
            max_height: 2048,
            max_fps: 120,
            screen_width_px: 1290,
            screen_height_px: 2796,
            audio_opus: true,
        }),
        path: pb::PathKind::Intranet as i32,
        transport: pb::Transport::Tcp as i32,
    });

    let server_hello = pb::Envelope::from(pb::ServerHello {
        protocol: 1,
        host_version: "0.3.2".into(),
        features: 0x0000_0000_0000_2021,
        required_features: 0x0000_0000_0000_0001,
        host_id: "host-42".into(),
        host_name: "Studio Mac mini".into(),
        os: pb::Platform::Macos as i32,
        os_version: "macOS 26.3".into(),
        displays: vec![display()],
        peer: Some(pb::PeerInfo {
            user: "alice@".into(),
            node: "xl-iphone".into(),
            device_id: "device-07".into(),
            path: pb::PathKind::TailnetDirect as i32,
            derp_region: "Shanghai".into(),
        }),
    });

    let auth_result = pb::Envelope::from(pb::AuthResult {
        status: pb::auth_result::Status::Ok as i32,
        role: pb::Role::Controller as i32,
        session_token: vec![0xab; 16],
        approval_timeout_s: 30,
        error: Some(pb::ErrorInfo {
            code: 140,
            message: "approval timed out".into(),
            retry_after_s: 7,
            detail: [("reason".into(), "timeout".into())].into(),
        }),
        resumed: true,
    });

    let session_state = pb::Envelope::from(pb::SessionState {
        your_role: pb::Role::Viewer as i32,
        controller: Some(participant()),
        viewers: vec![pb::Participant {
            device_id: "device-08".into(),
            device_name: "Office iPad".into(),
            user: "guest@".into(),
            path: pb::PathKind::Lan as i32,
            since_us: 1_726_000_000_654_321,
        }],
        input_paused_local: true,
    });

    let start_stream = pb::Envelope::from(pb::StartStream {
        display_id: 7,
        codec: pb::Codec::H264 as i32,
        max_width: 2560,
        max_height: 1440,
        fps: 120,
        bitrate_kbps: 12_000,
        quality: pb::Quality::Eco as i32,
    });

    let stream_config = pb::Envelope::from(pb::StreamConfig {
        epoch: 9,
        display_id: 7,
        codec: pb::Codec::Hevc as i32,
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 8000,
        cursor_x: 0.25,
        cursor_y: 0.75,
        cursor_visible: true,
        ..Default::default()
    });

    let keyframe_request = pb::Envelope::from(pb::KeyframeRequest {
        reason: pb::keyframe_request::Reason::Backlog as i32,
        last_good_frame: 1234,
    });

    let feedback = pb::Envelope::from(pb::Feedback {
        epoch: 9,
        interval_ms: 200,
        frames_received: 120,
        frames_lost: 2,
        frames_recovered: 3,
        shards_received: 1200,
        shards_lost: 15,
        recv_kbps: 7900,
        delay_gradient_us_per_s: -25_000,
        owd_p50_us: 24_000,
        decode_queue: 1,
        decode_p50_us: 9000,
        render_fps: 60,
        battery_pct: 73,
        low_power: true,
    });

    let rate_update = pb::Envelope::from(pb::RateUpdate {
        target_kbps: 6200,
        fps: 45,
        scale: 0.75,
        fec_ratio_pct: 18,
        reason: "path:derp".into(),
    });

    let ping = pb::Envelope::from(pb::Ping {
        t1_us: 1_726_000_000_111_111,
    });
    let pong = pb::Envelope::from(pb::Pong {
        t1_us: 1_726_000_000_111_111,
        t2_us: 1_726_000_000_111_999,
    });

    let display_list = pb::Envelope::from(pb::DisplayList {
        displays: vec![display()],
    });
    let select_display = pb::Envelope::from(pb::SelectDisplay { display_id: 7 });
    let request_control = pb::Envelope::from(pb::RequestControl {});
    let control_request = pb::Envelope::from(pb::ControlRequest {
        requester: Some(participant()),
        timeout_s: 30,
    });
    let control_response = pb::Envelope::from(pb::ControlResponse {
        requester_device_id: "device-07".into(),
        allow: true,
    });
    let local_activity = pb::Envelope::from(pb::LocalActivity { active: true });
    let idle_warning = pb::Envelope::from(pb::IdleWarning {
        seconds_left: 60,
        lock_on_end: true,
    });
    let pause_stream = pb::Envelope::from(pb::PauseStream {});
    let resume_stream = pb::Envelope::from(pb::ResumeStream {});
    let host_stats = pb::Envelope::from(pb::HostStats {
        capture_fps: 60,
        encode_p50_us: 5500,
        encode_p95_us: 9800,
        cpu_pct: 37,
        path: pb::PathKind::Manual as i32,
        derp_region: "Tokyo".into(),
        send_kbps: 7800,
    });
    let session_action = pb::Envelope::from(pb::SessionAction {
        kind: pb::session_action::Kind::CtrlAltDel as i32,
        enable: true,
    });
    let set_quality = pb::Envelope::from(pb::SetQuality {
        quality: pb::Quality::Smooth as i32,
    });
    let attach_input = pb::Envelope::from(pb::AttachInput {
        session_token: vec![0xcd; 16],
    });

    let pair_start = pb::Envelope::from(pb::PairStart {
        device_name: "New iPad".into(),
        platform: pb::Platform::Ipados as i32,
        has_qr: true,
        ..Default::default()
    });
    let pair_spake = pb::Envelope::from(pb::PairSpake {
        msg: vec![0x42; 32],
    });
    let pair_confirm = pb::Envelope::from(pb::PairConfirm {
        mac: vec![0x24; 32],
    });
    let pair_result = pb::Envelope::from(pb::PairResult {
        status: pb::pair_result::Status::Locked as i32,
        host_id: "host-42".into(),
        host_name: "Studio Mac mini".into(),
        attempts_left: 2,
        ..Default::default()
    });
    let pair_password = pb::Envelope::from(pb::PairPassword {
        device_name: "New iPad".into(),
        platform: pb::Platform::Ipados as i32,
        account: "alice".into(),
        password: b"correct horse".to_vec(),
    });

    let error_info = pb::Envelope::from(pb::ErrorInfo {
        code: 171,
        message: "app version is too old".into(),
        retry_after_s: 15,
        detail: [("min".into(), "0.3.0".into())].into(),
    });
    let goodbye = pb::Envelope::from(pb::Goodbye {
        reason: pb::goodbye::Reason::Revoked as i32,
        message: "device revoked".into(),
    });

    vec![
        (
            "file_list",
            pb::Envelope::from(pb::FileList {
                request_id: 1,
                path: "/Users/example/Downloads".into(),
                offset: 1000,
            }),
            json!({"name": "file_list", "fields": {"request_id": 1, "path": "/Users/example/Downloads", "offset": 1000}, "non_default_enums": [], "binary": "file_list.bin", "description": "M2 file channel contract."}),
        ),
        (
            "file_listing",
            pb::Envelope::from(pb::FileListing {
                request_id: 1,
                path: "/Users/example/Downloads".into(),
                parent: String::new(),
                entries: vec![pb::FileEntry {
                    name: "hello.txt".into(),
                    kind: pb::file_entry::Kind::File as i32,
                    size: 7,
                    mtime_us: 9,
                    child_count: -1,
                }],
                next_offset: 1,
                error: 0,
            }),
            json!({"name": "file_listing", "fields": {"request_id": 1, "path": "/Users/example/Downloads", "parent": "", "entries": [{"name": "hello.txt", "kind": "FILE", "size": 7, "mtime_us": 9, "child_count": -1}], "next_offset": 1, "error": "FILE_ERROR_NONE"}, "non_default_enums": ["FileEntry.Kind::FILE"], "binary": "file_listing.bin", "description": "M2 file channel contract."}),
        ),
        (
            "file_request",
            pb::Envelope::from(pb::FileRequest {
                request_id: 2,
                path: "/Users/example/Downloads/hello.txt".into(),
                transfer_id: "00112233445566778899aabbccddeeff".into(),
                offset: 3,
                expect_size: 7,
                expect_mtime_us: 9,
                expect_sha256: vec![0xab; 32],
            }),
            json!({"name": "file_request", "fields": {"request_id": 2, "path": "/Users/example/Downloads/hello.txt", "transfer_id": "00112233445566778899aabbccddeeff", "offset": 3, "expect_size": 7, "expect_mtime_us": 9, "expect_sha256_hex": "abababababababababababababababababababababababababababababababab"}, "non_default_enums": [], "binary": "file_request.bin", "description": "M2 file channel contract."}),
        ),
        (
            "file_query",
            pb::Envelope::from(pb::FileQuery {
                request_id: 3,
                transfer_id: "00112233445566778899aabbccddeeff".into(),
            }),
            json!({"name": "file_query", "fields": {"request_id": 3, "transfer_id": "00112233445566778899aabbccddeeff"}, "non_default_enums": [], "binary": "file_query.bin", "description": "M2 file channel contract."}),
        ),
        (
            "file_status",
            pb::Envelope::from(pb::FileStatus {
                request_id: 3,
                transfer_id: "00112233445566778899aabbccddeeff".into(),
                state: pb::file_status::State::Completed as i32,
                received: 7,
                error: 0,
                saved_name: "hello (1).txt".into(),
            }),
            json!({"name": "file_status", "fields": {"request_id": 3, "transfer_id": "00112233445566778899aabbccddeeff", "state": "COMPLETED", "received": 7, "error": "FILE_ERROR_NONE", "saved_name": "hello (1).txt"}, "non_default_enums": ["FileStatus.State::COMPLETED"], "binary": "file_status.bin", "description": "M2 file channel contract."}),
        ),
        (
            "file_cancel",
            pb::Envelope::from(pb::FileCancel {
                transfer_id: "00112233445566778899aabbccddeeff".into(),
            }),
            json!({"name": "file_cancel", "fields": {"transfer_id": "00112233445566778899aabbccddeeff"}, "non_default_enums": [], "binary": "file_cancel.bin", "description": "M2 file channel contract."}),
        ),
        (
            "client_hello",
            client_hello,
            json!({
                "name": "client_hello",
                "fields": {
                    "protocol": 1,
                    "app_version": "0.3.1",
                    "features": 8203,
                    "required_features": 1,
                    "device_name": "Alice's iPhone",
                    "platform": "IOS",
                    "intent": "PAIR",
                    "resume_token_hex": "11111111111111111111111111111111",
                    "caps": {
                        "decode": ["HEVC", "H264"],
                        "max_width": 2732,
                        "max_height": 2048,
                        "max_fps": 120,
                        "screen_width_px": 1290,
                        "screen_height_px": 2796,
                        "audio_opus": true
                    },
                    "path": "INTRANET",
                    "transport": "TCP"
                },
                "non_default_enums": ["Platform::IOS", "Intent::PAIR", "PathKind::INTRANET", "Transport::TCP", "Codec::HEVC", "Codec::H264"],
                "binary": "client_hello.bin",
                "description": "ClientHello with nested capabilities and every enum set away from its default."
            }),
        ),
        (
            "server_hello",
            server_hello,
            json!({
                "name": "server_hello",
                "fields": {
                    "protocol": 1,
                    "host_version": "0.3.2",
                    "features": 8225,
                    "required_features": 1,
                    "host_id": "host-42",
                    "host_name": "Studio Mac mini",
                    "os": "MACOS",
                    "os_version": "macOS 26.3",
                    "displays": [display_json()],
                    "peer": {
                        "user": "alice@",
                        "node": "xl-iphone",
                        "device_id": "device-07",
                        "path": "TAILNET_DIRECT",
                        "derp_region": "Shanghai"
                    }
                },
                "non_default_enums": ["Platform::MACOS", "PathKind::TAILNET_DIRECT"],
                "binary": "server_hello.bin",
                "description": "ServerHello carrying a display and PeerInfo."
            }),
        ),
        (
            "auth_result",
            auth_result,
            json!({
                "name": "auth_result",
                "fields": {
                    "status": "OK",
                    "role": "CONTROLLER",
                    "session_token_hex": "abababababababababababababababab",
                    "approval_timeout_s": 30,
                    "error": {"code": 140, "message": "approval timed out", "retry_after_s": 7, "detail": {"reason": "timeout"}},
                    "resumed": true
                },
                "non_default_enums": ["AuthResult.Status::OK", "Role::CONTROLLER"],
                "binary": "auth_result.bin",
                "description": "Successful AuthResult with token, role, error payload and resumed flag populated."
            }),
        ),
        (
            "session_state",
            session_state,
            json!({
                "name": "session_state",
                "fields": {
                    "your_role": "VIEWER",
                    "controller": participant_json(),
                    "viewers": [{
                        "device_id": "device-08",
                        "device_name": "Office iPad",
                        "user": "guest@",
                        "path": "LAN",
                        "since_us": 1726000000654321u64
                    }],
                    "input_paused_local": true
                },
                "non_default_enums": ["Role::VIEWER", "PathKind::TAILNET_DERP", "PathKind::LAN"],
                "binary": "session_state.bin",
                "description": "Viewer session with a controller and one additional viewer."
            }),
        ),
        (
            "start_stream",
            start_stream,
            json!({
                "name": "start_stream",
                "fields": {"display_id": 7, "codec": "H264", "max_width": 2560, "max_height": 1440, "fps": 120, "bitrate_kbps": 12000, "quality": "ECO"},
                "non_default_enums": ["Codec::H264", "Quality::ECO"],
                "binary": "start_stream.bin",
                "description": "StartStream requesting H.264 at the ECO quality tier."
            }),
        ),
        (
            "stream_config",
            stream_config,
            json!({
                "name": "stream_config",
                "fields": {"epoch": 9, "display_id": 7, "codec": "HEVC", "width": 1920, "height": 1080, "fps": 60, "bitrate_kbps": 8000, "cursor_x": 0.25, "cursor_y": 0.75, "cursor_visible": true},
                "non_default_enums": ["Codec::HEVC"],
                "binary": "stream_config.bin",
                "description": "StreamConfig for epoch 9 with a visible cursor."
            }),
        ),
        (
            "keyframe_request",
            keyframe_request,
            json!({
                "name": "keyframe_request",
                "fields": {"reason": "BACKLOG", "last_good_frame": 1234},
                "non_default_enums": ["KeyframeRequest.Reason::BACKLOG"],
                "binary": "keyframe_request.bin",
                "description": "Keyframe request after a decoder backlog."
            }),
        ),
        (
            "feedback",
            feedback,
            json!({
                "name": "feedback",
                "fields": {"epoch": 9, "interval_ms": 200, "frames_received": 120, "frames_lost": 2, "frames_recovered": 3, "shards_received": 1200, "shards_lost": 15, "recv_kbps": 7900, "delay_gradient_us_per_s": -25000, "owd_p50_us": 24000, "decode_queue": 1, "decode_p50_us": 9000, "render_fps": 60, "battery_pct": 73, "low_power": true},
                "non_default_enums": [],
                "binary": "feedback.bin",
                "description": "A full Feedback sample with negative delay gradient and low-power mode."
            }),
        ),
        (
            "rate_update",
            rate_update,
            json!({
                "name": "rate_update",
                "fields": {"target_kbps": 6200, "fps": 45, "scale": 0.75, "fec_ratio_pct": 18, "reason": "path:derp"},
                "non_default_enums": [],
                "binary": "rate_update.bin",
                "description": "RateUpdate after the path changed to DERP."
            }),
        ),
        (
            "ping",
            ping,
            json!({
                "name": "ping",
                "fields": {"t1_us": 1726000000111111u64},
                "non_default_enums": [],
                "binary": "ping.bin",
                "description": "Ping with a fixed Unix microsecond send timestamp."
            }),
        ),
        (
            "pong",
            pong,
            json!({
                "name": "pong",
                "fields": {"t1_us": 1726000000111111u64, "t2_us": 1726000000111999u64},
                "non_default_enums": [],
                "binary": "pong.bin",
                "description": "Pong echoing Ping.t1 and adding the host timestamp."
            }),
        ),
        (
            "display_list",
            display_list,
            json!({
                "name": "display_list",
                "fields": {"displays": [display_json()]},
                "non_default_enums": [],
                "binary": "display_list.bin",
                "description": "DisplayList containing a rotated primary display."
            }),
        ),
        (
            "select_display",
            select_display,
            json!({
                "name": "select_display",
                "fields": {"display_id": 7},
                "non_default_enums": [],
                "binary": "select_display.bin",
                "description": "SelectDisplay targeting display 7."
            }),
        ),
        (
            "request_control",
            request_control,
            json!({
                "name": "request_control",
                "fields": {},
                "non_default_enums": [],
                "binary": "request_control.bin",
                "description": "Empty RequestControl marker message."
            }),
        ),
        (
            "control_request",
            control_request,
            json!({
                "name": "control_request",
                "fields": {"requester": participant_json(), "timeout_s": 30},
                "non_default_enums": ["PathKind::TAILNET_DERP"],
                "binary": "control_request.bin",
                "description": "ControlRequest asking the current controller to hand off."
            }),
        ),
        (
            "control_response",
            control_response,
            json!({
                "name": "control_response",
                "fields": {"requester_device_id": "device-07", "allow": true},
                "non_default_enums": [],
                "binary": "control_response.bin",
                "description": "ControlResponse allowing device-07 to take control."
            }),
        ),
        (
            "local_activity",
            local_activity,
            json!({
                "name": "local_activity",
                "fields": {"active": true},
                "non_default_enums": [],
                "binary": "local_activity.bin",
                "description": "LocalActivity indicating someone is using the host."
            }),
        ),
        (
            "idle_warning",
            idle_warning,
            json!({
                "name": "idle_warning",
                "fields": {"seconds_left": 60, "lock_on_end": true},
                "non_default_enums": [],
                "binary": "idle_warning.bin",
                "description": "IdleWarning with 60 seconds remaining and lock-on-end enabled."
            }),
        ),
        (
            "pause_stream",
            pause_stream,
            json!({
                "name": "pause_stream",
                "fields": {},
                "non_default_enums": [],
                "binary": "pause_stream.bin",
                "description": "Empty PauseStream marker message."
            }),
        ),
        (
            "resume_stream",
            resume_stream,
            json!({
                "name": "resume_stream",
                "fields": {},
                "non_default_enums": [],
                "binary": "resume_stream.bin",
                "description": "Empty ResumeStream marker message."
            }),
        ),
        (
            "host_stats",
            host_stats,
            json!({
                "name": "host_stats",
                "fields": {"capture_fps": 60, "encode_p50_us": 5500, "encode_p95_us": 9800, "cpu_pct": 37, "path": "MANUAL", "derp_region": "Tokyo", "send_kbps": 7800},
                "non_default_enums": ["PathKind::MANUAL"],
                "binary": "host_stats.bin",
                "description": "HostStats with a manual path and non-empty DERP region."
            }),
        ),
        (
            "session_action",
            session_action,
            json!({
                "name": "session_action",
                "fields": {"kind": "CTRL_ALT_DEL", "enable": true},
                "non_default_enums": ["SessionAction.Kind::CTRL_ALT_DEL"],
                "binary": "session_action.bin",
                "description": "SessionAction requesting Ctrl+Alt+Del."
            }),
        ),
        (
            "set_quality",
            set_quality,
            json!({
                "name": "set_quality",
                "fields": {"quality": "SMOOTH"},
                "non_default_enums": ["Quality::SMOOTH"],
                "binary": "set_quality.bin",
                "description": "SetQuality selecting the smooth quality tier."
            }),
        ),
        (
            "set_audio",
            pb::Envelope::from(pb::SetAudio { enabled: true }),
            json!({
                "name": "set_audio",
                "fields": {"enabled": true},
                "non_default_enums": [],
                "binary": "set_audio.bin",
                "description": "SetAudio enabling remote audio."
            }),
        ),
        (
            "recording_notice",
            pb::Envelope::from(pb::RecordingNotice { active: true }),
            json!({
                "name": "recording_notice",
                "fields": {"active": true},
                "non_default_enums": [],
                "binary": "recording_notice.bin",
                "description": "RecordingNotice announcing that the App started local recording."
            }),
        ),
        (
            "create_virtual_display",
            pb::CreateVirtualDisplay {
                request_id: 1,
                width_px: 2732,
                height_px: 2048,
                scale: 2.0,
                refresh_hz: 60,
                select: true,
            }
            .into(),
            json!({
                "name": "create_virtual_display",
                "fields": {"request_id": 1, "width_px": 2732, "height_px": 2048, "scale": 2.0, "refresh_hz": 60, "select": true},
                "non_default_enums": [],
                "binary": "create_virtual_display.bin",
                "description": "CreateVirtualDisplay requesting an iPad-sized virtual display."
            }),
        ),
        (
            "virtual_display_result",
            pb::VirtualDisplayResult {
                request_id: 1,
                status: pb::virtual_display_result::Status::Ok as i32,
                display: Some(pb::Display {
                    id: 3,
                    name: "Virtual".into(),
                    width_px: 2732,
                    height_px: 2048,
                    scale: 2.0,
                    x: 1512,
                    y: 0,
                    refresh_hz: 60,
                    primary: false,
                    rotation: 0,
                    is_virtual: true,
                }),
                message: String::new(),
            }
            .into(),
            json!({
                "name": "virtual_display_result",
                "fields": {"request_id": 1, "status": "OK", "display": {"id": 3, "name": "Virtual", "width_px": 2732, "height_px": 2048, "scale": 2.0, "x": 1512, "y": 0, "refresh_hz": 60, "is_virtual": true}, "message": ""},
                "non_default_enums": ["VirtualDisplayResult.Status::OK"],
                "binary": "virtual_display_result.bin",
                "description": "VirtualDisplayResult confirming a virtual display."
            }),
        ),
        (
            "release_virtual_display",
            pb::ReleaseVirtualDisplay { display_id: 3 }.into(),
            json!({
                "name": "release_virtual_display",
                "fields": {"display_id": 3},
                "non_default_enums": [],
                "binary": "release_virtual_display.bin",
                "description": "ReleaseVirtualDisplay removing the session's virtual display."
            }),
        ),
        (
            "viewport",
            pb::Envelope::from(pb::Viewport {
                seq: 7,
                display_id: 1,
                rect: Some(pb::PixelRect {
                    x: 480,
                    y: 256,
                    w: 960,
                    h: 560,
                }),
                settled: true,
                view_width_px: 2532,
                view_height_px: 1170,
                crop_ok: true,
            }),
            json!({
                "name": "viewport",
                "fields": {"seq": 7, "display_id": 1, "rect": {"x": 480, "y": 256, "w": 960, "h": 560}, "settled": true, "view_width_px": 2532, "view_height_px": 1170, "crop_ok": true},
                "non_default_enums": [],
                "binary": "viewport.bin",
                "description": "Viewport reporting a settled 2x zoom on a 1920x1080 display."
            }),
        ),
        (
            "viewport_ack",
            pb::Envelope::from(pb::ViewportAck {
                seq: 7,
                mode: pb::viewport_ack::Mode::Crop as i32,
            }),
            json!({
                "name": "viewport_ack",
                "fields": {"seq": 7, "mode": "CROP"},
                "non_default_enums": ["ViewportAck.Mode::CROP"],
                "binary": "viewport_ack.bin",
                "description": "ViewportAck confirming crop mode."
            }),
        ),
        (
            "attach_input",
            attach_input,
            json!({
                "name": "attach_input",
                "fields": {"session_token_hex": "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"},
                "non_default_enums": [],
                "binary": "attach_input.bin",
                "description": "AttachInput carrying a 16-byte session token."
            }),
        ),
        (
            "pair_start",
            pair_start,
            json!({
                "name": "pair_start",
                "fields": {"device_name": "New iPad", "platform": "IPADOS", "has_qr": true},
                "non_default_enums": ["Platform::IPADOS"],
                "binary": "pair_start.bin",
                "description": "PairStart from an iPad using the QR path."
            }),
        ),
        (
            "pair_spake",
            pair_spake,
            json!({
                "name": "pair_spake",
                "fields": {"msg_hex": "4242424242424242424242424242424242424242424242424242424242424242"},
                "non_default_enums": [],
                "binary": "pair_spake.bin",
                "description": "PairSpake carrying a 32-byte opaque SPAKE2 message."
            }),
        ),
        (
            "pair_confirm",
            pair_confirm,
            json!({
                "name": "pair_confirm",
                "fields": {"mac_hex": "2424242424242424242424242424242424242424242424242424242424242424"},
                "non_default_enums": [],
                "binary": "pair_confirm.bin",
                "description": "PairConfirm carrying a 32-byte HMAC."
            }),
        ),
        (
            "pair_result",
            pair_result,
            json!({
                "name": "pair_result",
                "fields": {"status": "LOCKED", "host_id": "host-42", "host_name": "Studio Mac mini", "attempts_left": 2},
                "non_default_enums": ["PairResult.Status::LOCKED"],
                "binary": "pair_result.bin",
                "description": "PairResult reporting a locked pairing attempt."
            }),
        ),
        (
            "pair_password",
            pair_password,
            json!({
                "name": "pair_password",
                "fields": {"device_name": "New iPad", "platform": "IPADOS", "account": "alice", "password": "636f727265637420686f727365"},
                "non_default_enums": ["Platform::IPADOS"],
                "binary": "pair_password.bin",
                "description": "PairPassword with test-only fake credentials; the password is hex-encoded. Real implementations must never log the password."
            }),
        ),
        (
            "error",
            error_info,
            json!({
                "name": "error",
                "fields": {"code": 171, "message": "app version is too old", "retry_after_s": 15, "detail": {"min": "0.3.0"}},
                "non_default_enums": [],
                "binary": "error.bin",
                "description": "ErrorInfo for AppTooOld with a minimum version detail."
            }),
        ),
        (
            "goodbye",
            goodbye,
            json!({
                "name": "goodbye",
                "fields": {"reason": "REVOKED", "message": "device revoked"},
                "non_default_enums": ["Goodbye.Reason::REVOKED"],
                "binary": "goodbye.bin",
                "description": "Goodbye for a revoked device."
            }),
        ),
    ]
}

fn display_json() -> Value {
    json!({
        "id": 7,
        "name": "DELL U2723QE",
        "width_px": 3840,
        "height_px": 2160,
        "scale": 1.5,
        "x": -1920,
        "y": 120,
        "refresh_hz": 60,
        "primary": true,
        "rotation": 90
    })
}

fn participant_json() -> Value {
    json!({
        "device_id": "device-07",
        "device_name": "Alice's iPad",
        "user": "alice@",
        "path": "TAILNET_DERP",
        "since_us": 1726000000123456u64
    })
}

fn is_regen() -> bool {
    std::env::var_os("RSP_REGEN_VECTORS").is_some_and(|value| value == "1")
}

fn sorted_files(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .expect("vector directory exists")
        .map(|entry| entry.expect("vector directory entry").path())
        .map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .expect("UTF-8 vector file name")
                .to_owned()
        })
        .collect()
}

fn ensure_vector_files(dir: &Path, cases: &[(&str, pb::Envelope, Value)]) {
    let _guard = VECTOR_FILES_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let registered: BTreeSet<String> = cases
        .iter()
        .map(|(name, _, _)| (*name).to_owned())
        .collect();
    assert_eq!(registered.len(), cases.len(), "duplicate vector name");
    for (name, envelope, _) in cases {
        assert_eq!(
            envelope_name(envelope),
            *name,
            "vector name disagrees with oneof member"
        );
    }

    let files = sorted_files(dir);
    let expected: BTreeSet<String> = registered
        .iter()
        .flat_map(|name| [format!("{name}.bin"), format!("{name}.json")])
        .collect();
    if is_regen() {
        fs::create_dir_all(dir).expect("create control vector directory");
        for (name, env, description) in cases {
            fs::write(dir.join(format!("{name}.bin")), encode(env)).expect("write vector binary");
            let mut pretty = serde_json::to_vec_pretty(description).expect("serialize vector json");
            pretty.push(b'\n');
            fs::write(dir.join(format!("{name}.json")), pretty).expect("write vector json");
        }
        for stale in files {
            if !expected.contains(&stale) {
                let _ = fs::remove_file(dir.join(stale));
            }
        }
    } else {
        assert_eq!(
            files,
            expected,
            "control vector file set drifted; expected one .bin/.json pair for each of {}",
            registered.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }
}

#[test]
fn control_vectors_cover_every_oneof_member() {
    let dir = vector_dir();
    let cases = vectors();
    assert_eq!(
        cases.len(),
        CONTROL_ONEOF_MEMBERS,
        "Envelope oneof member count changed"
    );
    for (name, envelope, _) in &cases {
        assert_eq!(
            envelope_name(envelope),
            *name,
            "vector name disagrees with oneof member"
        );
    }
    ensure_vector_files(&dir, &cases);
}

#[test]
fn control_vectors_decode_and_reencode_exactly() {
    let dir = vector_dir();
    let cases = vectors();
    ensure_vector_files(&dir, &cases);
    let members: BTreeSet<_> = cases
        .iter()
        .map(|(name, envelope, _)| (envelope_name(envelope), *name))
        .collect();
    assert_eq!(members.len(), cases.len(), "duplicate oneof member vector");
    for (name, expected, description) in cases {
        let bin = fs::read(dir.join(format!("{name}.bin")))
            .unwrap_or_else(|error| panic!("read {name}.bin: {error}"));
        let raw_json = fs::read(dir.join(format!("{name}.json")))
            .unwrap_or_else(|error| panic!("read {name}.json: {error}"));
        let json: Value = serde_json::from_slice(&raw_json)
            .unwrap_or_else(|error| panic!("parse {name}.json: {error}"));

        assert_eq!(json["name"], name, "vector JSON name differs for {name}");
        assert_eq!(
            json["binary"],
            format!("{name}.bin"),
            "vector JSON points at the wrong binary for {name}"
        );
        assert!(
            json["non_default_enums"].is_array(),
            "vector {name} has no non-default enum documentation"
        );
        for value in json["non_default_enums"]
            .as_array()
            .unwrap_or_else(|| panic!("vector {name} has no enum documentation array"))
        {
            assert!(
                value.is_string(),
                "vector {name} has a non-string enum name"
            );
        }
        assert_eq!(json, description, "vector description drifted for {name}");

        assert_eq!(encode(&expected), bin, "encode differs from {name}.bin");
        let mut decoder = EnvelopeDecoder::new();
        decoder.push(&bin);
        assert_eq!(
            decoder.next().expect("vector decodes").as_ref(),
            Some(&expected),
            "decoded value differs for {name}"
        );
        assert_eq!(decoder.next().expect("no trailing bytes"), None);

        let payload_len = u32::from_be_bytes(bin[..4].try_into().expect("length header")) as usize;
        assert_eq!(
            payload_len + 4,
            bin.len(),
            "length header mismatch for {name}"
        );
        let envelope = pb::Envelope::decode(&bin[4..]).expect("payload is a protobuf Envelope");
        assert_eq!(envelope, expected, "protobuf payload differs for {name}");
    }
}
