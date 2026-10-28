use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;

use rsp_core::control::{encode, EnvelopeDecoder};
use rsp_core::pb;
use serde_json::{json, Value};

const ENUM_VALUES: [(&str, &[&str]); 17] = [
    (
        "FileError",
        &[
            "FILE_ERROR_NOT_FOUND",
            "FILE_ERROR_PERMISSION_DENIED",
            "FILE_ERROR_OUTSIDE_ROOT",
            "FILE_ERROR_NOT_A_DIR",
            "FILE_ERROR_NOT_A_FILE",
            "FILE_ERROR_DISABLED",
            "FILE_ERROR_NOT_ALLOWED",
            "FILE_ERROR_BUSY",
            "FILE_ERROR_IO",
            "FILE_ERROR_NO_SPACE",
            "FILE_ERROR_TOO_LARGE",
            "FILE_ERROR_RESUME_MISMATCH",
            "FILE_ERROR_SHA256_MISMATCH",
            "FILE_ERROR_SOURCE_CHANGED",
            "FILE_ERROR_PROTOCOL",
        ],
    ),
    ("FileEntry.Kind", &["FILE", "DIR", "SYMLINK", "OTHER"]),
    (
        "FileStatus.State",
        &[
            "UNKNOWN",
            "PARTIAL",
            "RECEIVING",
            "COMPLETED",
            "FAILED",
            "CANCELLED",
        ],
    ),
    (
        "Platform",
        &["IOS", "IPADOS", "ANDROID", "MACOS", "WINDOWS", "LINUX"],
    ),
    ("Intent", &["CONTROL", "VIEW", "PAIR", "PROBE", "FILES"]),
    (
        "PathKind",
        &[
            "TAILNET_DIRECT",
            "TAILNET_DERP",
            "INTRANET",
            "LAN",
            "MANUAL",
        ],
    ),
    ("Transport", &["QUIC", "TCP"]),
    ("Codec", &["HEVC", "H264"]),
    ("Role", &["CONTROLLER", "VIEWER"]),
    ("Quality", &["CLEAR", "SMOOTH", "ECO"]),
    ("AuthResult.Status", &["OK", "PENDING_APPROVAL", "DENIED"]),
    (
        "SessionAction.Kind",
        &["LOCK_SCREEN", "CTRL_ALT_DEL", "LOCK_ON_END", "WAKE_DISPLAY"],
    ),
    (
        "KeyframeRequest.Reason",
        &["LOSS", "DECODER_RESET", "RESUME", "BACKLOG"],
    ),
    (
        "PairResult.Status",
        &[
            "OK",
            "WAITING_LOCAL",
            "DENIED",
            "BAD_CODE",
            "EXPIRED",
            "LOCKED",
            "DISABLED_ON_PATH",
            "METHOD_DISABLED",
        ],
    ),
    (
        "Goodbye.Reason",
        &[
            "USER_CLOSED",
            "HOST_CLOSED_BY_LOCAL_USER",
            "IDLE_TIMEOUT",
            "REPLACED",
            "REVOKED",
            "BACKGROUND",
            "HOST_SHUTDOWN",
            "HOST_UPDATING",
            "PERMISSION_LOST",
        ],
    ),
    ("ViewportAck.Mode", &["ROI", "CROP"]),
    (
        "VirtualDisplayResult.Status",
        &[
            "OK",
            "UNSUPPORTED",
            "NOT_ALLOWED",
            "LIMIT",
            "BAD_REQUEST",
            "FAILED",
        ],
    ),
];

static VECTOR_FILES_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static REGEN_ONCE: Once = Once::new();

fn vector_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/vectors")
        .join(name)
}

fn client_hello(
    platform: pb::Platform,
    intent: pb::Intent,
    transport: pb::Transport,
    device_name: &str,
    width_px: u32,
    height_px: u32,
) -> pb::Envelope {
    pb::Envelope::from(pb::ClientHello {
        protocol: 1,
        app_version: "0.3.1".into(),
        features: 0x0000_0000_0000_2007,
        required_features: 0x0000_0000_0000_0001,
        device_name: device_name.into(),
        platform: platform as i32,
        intent: intent as i32,
        resume_token: Vec::new(),
        caps: Some(pb::ClientCaps {
            decode: vec![pb::Codec::Hevc as i32, pb::Codec::H264 as i32],
            max_width: width_px,
            max_height: height_px,
            max_fps: 120,
            screen_width_px: width_px,
            screen_height_px: height_px,
            audio_opus: true,
        }),
        path: pb::PathKind::Intranet as i32,
        transport: transport as i32,
    })
}

fn server_hello(os: pb::Platform, os_version: &str, host_name: &str) -> pb::Envelope {
    pb::Envelope::from(pb::ServerHello {
        protocol: 1,
        host_version: "0.3.2".into(),
        features: 0x0000_0000_0000_2021,
        required_features: 0x0000_0000_0000_0001,
        host_id: "host-42".into(),
        host_name: host_name.into(),
        os: os as i32,
        os_version: os_version.into(),
        displays: vec![pb::Display {
            id: 1,
            name: "Primary".into(),
            width_px: 2560,
            height_px: 1440,
            scale: 1.25,
            x: 0,
            y: 0,
            refresh_hz: 144,
            primary: true,
            rotation: 0,
            is_virtual: false,
        }],
        peer: None,
    })
}

fn session_action(kind: pb::session_action::Kind, enable: bool) -> pb::Envelope {
    pb::Envelope::from(pb::SessionAction {
        kind: kind as i32,
        enable,
    })
}

fn keyframe_request(reason: pb::keyframe_request::Reason) -> pb::Envelope {
    pb::Envelope::from(pb::KeyframeRequest {
        reason: reason as i32,
        last_good_frame: 4321,
    })
}

fn pair_result(status: pb::pair_result::Status, attempts_left: u32) -> pb::Envelope {
    pb::Envelope::from(pb::PairResult {
        status: status as i32,
        host_id: "host-42".into(),
        host_name: "Studio PC".into(),
        attempts_left,
        ..Default::default()
    })
}

fn goodbye(reason: pb::goodbye::Reason, message: &str) -> pb::Envelope {
    pb::Envelope::from(pb::Goodbye {
        reason: reason as i32,
        message: message.into(),
    })
}

fn vectors() -> Vec<(&'static str, pb::Envelope, Value)> {
    let mut cases = vec![
        (
            "client_hello_android_control_quic",
            client_hello(
                pb::Platform::Android,
                pb::Intent::Control,
                pb::Transport::Quic,
                "Pixel 9 Pro",
                1280,
                2856,
            ),
            json!({
                "name": "client_hello_android_control_quic",
                "fields": {
                    "protocol": 1,
                    "app_version": "0.3.1",
                    "features": 8203,
                    "required_features": 1,
                    "device_name": "Pixel 9 Pro",
                    "platform": "ANDROID",
                    "intent": "CONTROL",
                    "caps": {
                        "decode": ["HEVC", "H264"],
                        "max_width": 1280,
                        "max_height": 2856,
                        "max_fps": 120,
                        "screen_width_px": 1280,
                        "screen_height_px": 2856,
                        "audio_opus": true
                    },
                    "path": "INTRANET",
                    "transport": "QUIC"
                },
                "non_default_enums": ["Platform::ANDROID", "Intent::CONTROL", "PathKind::INTRANET", "Transport::QUIC", "Codec::HEVC", "Codec::H264"],
                "binary": "client_hello_android_control_quic.bin",
                "description": "Android ClientHello over QUIC requesting a control session."
            }),
        ),
        (
            "client_hello_view",
            client_hello(
                pb::Platform::Ios,
                pb::Intent::View,
                pb::Transport::Tcp,
                "iPhone 16 Pro",
                1206,
                2622,
            ),
            json!({
                "name": "client_hello_view",
                "fields": {
                    "protocol": 1,
                    "app_version": "0.3.1",
                    "features": 8203,
                    "required_features": 1,
                    "device_name": "iPhone 16 Pro",
                    "platform": "IOS",
                    "intent": "VIEW",
                    "caps": {
                        "decode": ["HEVC", "H264"],
                        "max_width": 1206,
                        "max_height": 2622,
                        "max_fps": 120,
                        "screen_width_px": 1206,
                        "screen_height_px": 2622,
                        "audio_opus": true
                    },
                    "path": "INTRANET",
                    "transport": "TCP"
                },
                "non_default_enums": ["Platform::IOS", "Intent::VIEW", "PathKind::INTRANET", "Transport::TCP", "Codec::HEVC", "Codec::H264"],
                "binary": "client_hello_view.bin",
                "description": "ClientHello for a view-only session."
            }),
        ),
        (
            "client_hello_probe",
            client_hello(
                pb::Platform::Linux,
                pb::Intent::Probe,
                pb::Transport::Tcp,
                "ThinkPad X1",
                2880,
                1800,
            ),
            json!({
                "name": "client_hello_probe",
                "fields": {
                    "protocol": 1,
                    "app_version": "0.3.1",
                    "features": 8203,
                    "required_features": 1,
                    "device_name": "ThinkPad X1",
                    "platform": "LINUX",
                    "intent": "PROBE",
                    "caps": {
                        "decode": ["HEVC", "H264"],
                        "max_width": 2880,
                        "max_height": 1800,
                        "max_fps": 120,
                        "screen_width_px": 2880,
                        "screen_height_px": 1800,
                        "audio_opus": true
                    },
                    "path": "INTRANET",
                    "transport": "TCP"
                },
                "non_default_enums": ["Platform::LINUX", "Intent::PROBE", "PathKind::INTRANET", "Transport::TCP", "Codec::HEVC", "Codec::H264"],
                "binary": "client_hello_probe.bin",
                "description": "Linux probe ClientHello that only discovers a host."
            }),
        ),
        (
            "server_hello_windows",
            server_hello(pb::Platform::Windows, "Windows 11 24H2", "Studio PC"),
            json!({
                "name": "server_hello_windows",
                "fields": {
                    "protocol": 1,
                    "host_version": "0.3.2",
                    "features": 8225,
                    "required_features": 1,
                    "host_id": "host-42",
                    "host_name": "Studio PC",
                    "os": "WINDOWS",
                    "os_version": "Windows 11 24H2",
                    "displays": [{
                        "id": 1,
                        "name": "Primary",
                        "width_px": 2560,
                        "height_px": 1440,
                        "scale": 1.25,
                        "x": 0,
                        "y": 0,
                        "refresh_hz": 144,
                        "primary": true,
                        "rotation": 0
                    }]
                },
                "non_default_enums": ["Platform::WINDOWS"],
                "binary": "server_hello_windows.bin",
                "description": "ServerHello from a Windows host."
            }),
        ),
        (
            "server_hello_linux",
            server_hello(pb::Platform::Linux, "Ubuntu 26.04 LTS", "Studio Linux"),
            json!({
                "name": "server_hello_linux",
                "fields": {
                    "protocol": 1,
                    "host_version": "0.3.2",
                    "features": 8225,
                    "required_features": 1,
                    "host_id": "host-42",
                    "host_name": "Studio Linux",
                    "os": "LINUX",
                    "os_version": "Ubuntu 26.04 LTS",
                    "displays": [{
                        "id": 1,
                        "name": "Primary",
                        "width_px": 2560,
                        "height_px": 1440,
                        "scale": 1.25,
                        "x": 0,
                        "y": 0,
                        "refresh_hz": 144,
                        "primary": true,
                        "rotation": 0
                    }]
                },
                "non_default_enums": ["Platform::LINUX"],
                "binary": "server_hello_linux.bin",
                "description": "ServerHello from a Linux host."
            }),
        ),
        (
            "set_quality_clear",
            pb::Envelope::from(pb::SetQuality {
                quality: pb::Quality::Clear as i32,
            }),
            json!({
                "name": "set_quality_clear",
                "fields": {"quality": "CLEAR"},
                "non_default_enums": ["Quality::CLEAR"],
                "binary": "set_quality_clear.bin",
                "description": "SetQuality selecting the clear quality tier."
            }),
        ),
        (
            "auth_result_pending",
            pb::Envelope::from(pb::AuthResult {
                status: pb::auth_result::Status::PendingApproval as i32,
                role: pb::Role::Controller as i32,
                session_token: Vec::new(),
                approval_timeout_s: 30,
                error: None,
                resumed: false,
            }),
            json!({
                "name": "auth_result_pending",
                "fields": {
                    "status": "PENDING_APPROVAL",
                    "role": "CONTROLLER",
                    "approval_timeout_s": 30
                },
                "non_default_enums": ["AuthResult.Status::PENDING_APPROVAL", "Role::CONTROLLER"],
                "binary": "auth_result_pending.bin",
                "description": "AuthResult waiting for local approval with a 30 second timeout."
            }),
        ),
        (
            "auth_result_denied",
            pb::Envelope::from(pb::AuthResult {
                status: pb::auth_result::Status::Denied as i32,
                role: pb::Role::Viewer as i32,
                session_token: Vec::new(),
                approval_timeout_s: 0,
                error: Some(pb::ErrorInfo {
                    code: 181,
                    message: "device is not paired with this host".into(),
                    retry_after_s: 0,
                    detail: HashMap::new(),
                }),
                resumed: false,
            }),
            json!({
                "name": "auth_result_denied",
                "fields": {
                    "status": "DENIED",
                    "role": "VIEWER",
                    "error": {
                        "code": 181,
                        "message": "device is not paired with this host",
                        "retry_after_s": 0,
                        "detail": {}
                    }
                },
                "non_default_enums": ["AuthResult.Status::DENIED", "Role::VIEWER"],
                "binary": "auth_result_denied.bin",
                "description": "AuthResult denying an unpaired device with ErrorInfo code 181."
            }),
        ),
        (
            "session_action_lock_screen",
            session_action(pb::session_action::Kind::LockScreen, false),
            json!({
                "name": "session_action_lock_screen",
                "fields": {"kind": "LOCK_SCREEN", "enable": false},
                "non_default_enums": ["SessionAction.Kind::LOCK_SCREEN"],
                "binary": "session_action_lock_screen.bin",
                "description": "SessionAction asking the host to lock its screen."
            }),
        ),
        (
            "session_action_lock_on_end",
            session_action(pb::session_action::Kind::LockOnEnd, true),
            json!({
                "name": "session_action_lock_on_end",
                "fields": {"kind": "LOCK_ON_END", "enable": true},
                "non_default_enums": ["SessionAction.Kind::LOCK_ON_END"],
                "binary": "session_action_lock_on_end.bin",
                "description": "SessionAction enabling lock-on-end for the session."
            }),
        ),
        (
            "session_action_wake_display",
            session_action(pb::session_action::Kind::WakeDisplay, false),
            json!({
                "name": "session_action_wake_display",
                "fields": {"kind": "WAKE_DISPLAY", "enable": false},
                "non_default_enums": ["SessionAction.Kind::WAKE_DISPLAY"],
                "binary": "session_action_wake_display.bin",
                "description": "SessionAction waking a sleeping display."
            }),
        ),
        (
            "keyframe_request_loss",
            keyframe_request(pb::keyframe_request::Reason::Loss),
            json!({
                "name": "keyframe_request_loss",
                "fields": {"reason": "LOSS", "last_good_frame": 4321},
                "non_default_enums": ["KeyframeRequest.Reason::LOSS"],
                "binary": "keyframe_request_loss.bin",
                "description": "KeyframeRequest after packet loss."
            }),
        ),
        (
            "keyframe_request_decoder_reset",
            keyframe_request(pb::keyframe_request::Reason::DecoderReset),
            json!({
                "name": "keyframe_request_decoder_reset",
                "fields": {"reason": "DECODER_RESET", "last_good_frame": 4321},
                "non_default_enums": ["KeyframeRequest.Reason::DECODER_RESET"],
                "binary": "keyframe_request_decoder_reset.bin",
                "description": "KeyframeRequest after resetting the decoder."
            }),
        ),
        (
            "keyframe_request_resume",
            keyframe_request(pb::keyframe_request::Reason::Resume),
            json!({
                "name": "keyframe_request_resume",
                "fields": {"reason": "RESUME", "last_good_frame": 4321},
                "non_default_enums": ["KeyframeRequest.Reason::RESUME"],
                "binary": "keyframe_request_resume.bin",
                "description": "KeyframeRequest after resuming a paused stream."
            }),
        ),
        (
            "viewport_ack_roi",
            pb::Envelope::from(pb::ViewportAck {
                seq: 1,
                mode: pb::viewport_ack::Mode::Roi as i32,
            }),
            json!({
                "name": "viewport_ack_roi",
                "fields": {"seq": 1, "mode": "ROI"},
                "non_default_enums": ["ViewportAck.Mode::ROI"],
                "binary": "viewport_ack_roi.bin",
                "description": "ViewportAck confirming ROI mode."
            }),
        ),
        (
            "viewport_ack_crop",
            pb::Envelope::from(pb::ViewportAck {
                seq: 2,
                mode: pb::viewport_ack::Mode::Crop as i32,
            }),
            json!({
                "name": "viewport_ack_crop",
                "fields": {"seq": 2, "mode": "CROP"},
                "non_default_enums": ["ViewportAck.Mode::CROP"],
                "binary": "viewport_ack_crop.bin",
                "description": "ViewportAck confirming crop mode."
            }),
        ),
        (
            "pair_result_ok",
            pair_result(pb::pair_result::Status::Ok, 0),
            json!({
                "name": "pair_result_ok",
                "fields": {"status": "OK", "host_id": "host-42", "host_name": "Studio PC", "attempts_left": 0},
                "non_default_enums": ["PairResult.Status::OK"],
                "binary": "pair_result_ok.bin",
                "description": "PairResult for a successful pairing."
            }),
        ),
        (
            "pair_result_waiting_local",
            pair_result(pb::pair_result::Status::WaitingLocal, 0),
            json!({
                "name": "pair_result_waiting_local",
                "fields": {"status": "WAITING_LOCAL", "host_id": "host-42", "host_name": "Studio PC", "attempts_left": 0},
                "non_default_enums": ["PairResult.Status::WAITING_LOCAL"],
                "binary": "pair_result_waiting_local.bin",
                "description": "PairResult waiting for local confirmation on the host."
            }),
        ),
        (
            "pair_result_denied",
            pair_result(pb::pair_result::Status::Denied, 0),
            json!({
                "name": "pair_result_denied",
                "fields": {"status": "DENIED", "host_id": "host-42", "host_name": "Studio PC", "attempts_left": 0},
                "non_default_enums": ["PairResult.Status::DENIED"],
                "binary": "pair_result_denied.bin",
                "description": "PairResult denied by the local user."
            }),
        ),
        (
            "pair_result_bad_code",
            pair_result(pb::pair_result::Status::BadCode, 4),
            json!({
                "name": "pair_result_bad_code",
                "fields": {"status": "BAD_CODE", "host_id": "host-42", "host_name": "Studio PC", "attempts_left": 4},
                "non_default_enums": ["PairResult.Status::BAD_CODE"],
                "binary": "pair_result_bad_code.bin",
                "description": "PairResult with a wrong pairing code and four attempts left."
            }),
        ),
        (
            "pair_result_expired",
            pair_result(pb::pair_result::Status::Expired, 0),
            json!({
                "name": "pair_result_expired",
                "fields": {"status": "EXPIRED", "host_id": "host-42", "host_name": "Studio PC", "attempts_left": 0},
                "non_default_enums": ["PairResult.Status::EXPIRED"],
                "binary": "pair_result_expired.bin",
                "description": "PairResult after the pairing window expired."
            }),
        ),
        (
            "pair_result_disabled_on_path",
            pair_result(pb::pair_result::Status::DisabledOnPath, 0),
            json!({
                "name": "pair_result_disabled_on_path",
                "fields": {"status": "DISABLED_ON_PATH", "host_id": "host-42", "host_name": "Studio PC", "attempts_left": 0},
                "non_default_enums": ["PairResult.Status::DISABLED_ON_PATH"],
                "binary": "pair_result_disabled_on_path.bin",
                "description": "PairResult reporting pairing disabled on the current path."
            }),
        ),
        (
            "pair_result_method_disabled",
            pair_result(pb::pair_result::Status::MethodDisabled, 0),
            json!({
                "name": "pair_result_method_disabled",
                "fields": {"status": "METHOD_DISABLED", "host_id": "host-42", "host_name": "Studio PC", "attempts_left": 0},
                "non_default_enums": ["PairResult.Status::METHOD_DISABLED"],
                "binary": "pair_result_method_disabled.bin",
                "description": "PairResult when this pairing method is turned off on the host."
            }),
        ),
        (
            "pair_result_locked_retry",
            pb::Envelope::from(pb::PairResult {
                status: pb::pair_result::Status::Locked as i32,
                retry_after_s: 873,
                ..Default::default()
            }),
            json!({
                "name": "pair_result_locked_retry",
                "fields": {"status": "LOCKED", "retry_after_s": 873},
                "non_default_enums": ["PairResult.Status::LOCKED"],
                "binary": "pair_result_locked_retry.bin",
                "description": "PairResult reporting a locked pairing attempt with seconds until retry."
            }),
        ),
        (
            "pair_start_file",
            pb::Envelope::from(pb::PairStart {
                device_name: "New iPad".into(),
                platform: pb::Platform::Ipados as i32,
                has_qr: false,
                pair_file_id: vec![0x11; 16],
            }),
            json!({
                "name": "pair_start_file",
                "fields": {"device_name": "New iPad", "platform": "IPADOS", "has_qr": false, "pair_file_id": "11111111111111111111111111111111"},
                "non_default_enums": ["Platform::IPADOS"],
                "binary": "pair_start_file.bin",
                "description": "PairStart from an iPad using a 16-byte pairing file identifier."
            }),
        ),
        (
            "goodbye_user_closed",
            goodbye(pb::goodbye::Reason::UserClosed, "closed by app"),
            json!({
                "name": "goodbye_user_closed",
                "fields": {"reason": "USER_CLOSED", "message": "closed by app"},
                "non_default_enums": ["Goodbye.Reason::USER_CLOSED"],
                "binary": "goodbye_user_closed.bin",
                "description": "Goodbye when the app user closes the session."
            }),
        ),
        (
            "goodbye_host_closed_by_local_user",
            goodbye(pb::goodbye::Reason::HostClosedByLocalUser, "closed on host"),
            json!({
                "name": "goodbye_host_closed_by_local_user",
                "fields": {"reason": "HOST_CLOSED_BY_LOCAL_USER", "message": "closed on host"},
                "non_default_enums": ["Goodbye.Reason::HOST_CLOSED_BY_LOCAL_USER"],
                "binary": "goodbye_host_closed_by_local_user.bin",
                "description": "Goodbye when someone at the host closes the session."
            }),
        ),
        (
            "goodbye_idle_timeout",
            goodbye(pb::goodbye::Reason::IdleTimeout, "idle for 30 minutes"),
            json!({
                "name": "goodbye_idle_timeout",
                "fields": {"reason": "IDLE_TIMEOUT", "message": "idle for 30 minutes"},
                "non_default_enums": ["Goodbye.Reason::IDLE_TIMEOUT"],
                "binary": "goodbye_idle_timeout.bin",
                "description": "Goodbye after the session idle timeout."
            }),
        ),
        (
            "goodbye_replaced",
            goodbye(pb::goodbye::Reason::Replaced, "another device took over"),
            json!({
                "name": "goodbye_replaced",
                "fields": {"reason": "REPLACED", "message": "another device took over"},
                "non_default_enums": ["Goodbye.Reason::REPLACED"],
                "binary": "goodbye_replaced.bin",
                "description": "Goodbye when a newer connection replaces this session."
            }),
        ),
        (
            "goodbye_background",
            goodbye(pb::goodbye::Reason::Background, "app moved to background"),
            json!({
                "name": "goodbye_background",
                "fields": {"reason": "BACKGROUND", "message": "app moved to background"},
                "non_default_enums": ["Goodbye.Reason::BACKGROUND"],
                "binary": "goodbye_background.bin",
                "description": "Goodbye when the app stays in the background too long."
            }),
        ),
        (
            "goodbye_host_shutdown",
            goodbye(pb::goodbye::Reason::HostShutdown, "host shutting down"),
            json!({
                "name": "goodbye_host_shutdown",
                "fields": {"reason": "HOST_SHUTDOWN", "message": "host shutting down"},
                "non_default_enums": ["Goodbye.Reason::HOST_SHUTDOWN"],
                "binary": "goodbye_host_shutdown.bin",
                "description": "Goodbye while the host shuts down."
            }),
        ),
        (
            "goodbye_host_updating",
            goodbye(pb::goodbye::Reason::HostUpdating, "host updating"),
            json!({
                "name": "goodbye_host_updating",
                "fields": {"reason": "HOST_UPDATING", "message": "host updating"},
                "non_default_enums": ["Goodbye.Reason::HOST_UPDATING"],
                "binary": "goodbye_host_updating.bin",
                "description": "Goodbye while the host installs an update."
            }),
        ),
        (
            "goodbye_permission_lost",
            goodbye(
                pb::goodbye::Reason::PermissionLost,
                "screen recording denied",
            ),
            json!({
                "name": "goodbye_permission_lost",
                "fields": {"reason": "PERMISSION_LOST", "message": "screen recording denied"},
                "non_default_enums": ["Goodbye.Reason::PERMISSION_LOST"],
                "binary": "goodbye_permission_lost.bin",
                "description": "Goodbye after the host loses a required permission."
            }),
        ),
    ];
    cases.extend(file_enum_vectors());
    for (name, status) in [
        (
            "virtual_display_result_ok",
            pb::virtual_display_result::Status::Ok,
        ),
        (
            "virtual_display_result_unsupported",
            pb::virtual_display_result::Status::Unsupported,
        ),
        (
            "virtual_display_result_not_allowed",
            pb::virtual_display_result::Status::NotAllowed,
        ),
        (
            "virtual_display_result_limit",
            pb::virtual_display_result::Status::Limit,
        ),
        (
            "virtual_display_result_bad_request",
            pb::virtual_display_result::Status::BadRequest,
        ),
        (
            "virtual_display_result_failed",
            pb::virtual_display_result::Status::Failed,
        ),
    ] {
        cases.push((
            name,
            pb::VirtualDisplayResult { request_id: 1, status: status as i32, ..Default::default() }.into(),
            json!({
                "name": name,
                "fields": {"request_id": 1, "status": status.as_str_name()},
                "non_default_enums": [format!("VirtualDisplayResult.Status::{}", status.as_str_name())],
                "binary": format!("{name}.bin"),
                "description": "M3 virtual display result status."
            }),
        ));
    }
    cases
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
    if is_regen() {
        fs::create_dir_all(dir).expect("create enum vector directory");
    }
    let files = sorted_files(dir);
    let expected: BTreeSet<String> = registered
        .iter()
        .flat_map(|name| [format!("{name}.bin"), format!("{name}.json")])
        .collect();
    if is_regen() {
        REGEN_ONCE.call_once(|| {
            for (name, env, description) in cases {
                fs::write(dir.join(format!("{name}.bin")), encode(env))
                    .expect("write vector binary");
                let mut pretty =
                    serde_json::to_vec_pretty(description).expect("serialize vector json");
                pretty.push(b'\n');
                fs::write(dir.join(format!("{name}.json")), pretty).expect("write vector json");
            }
            for stale in files {
                if !expected.contains(&stale) {
                    let _ = fs::remove_file(dir.join(stale));
                }
            }
        });
    } else {
        assert_eq!(
            files,
            expected,
            "control enum vector file set drifted; expected one .bin/.json pair for each of {}",
            registered.iter().cloned().collect::<Vec<_>>().join(", ")
        );
    }
}

#[test]
fn control_enum_vectors_match_files() {
    let dir = vector_dir("control_enums");
    let cases = vectors();
    assert_eq!(cases.len(), 65, "enum vector count changed");
    ensure_vector_files(&dir, &cases);

    for (name, expected, description) in cases {
        let bin = fs::read(dir.join(format!("{name}.bin")))
            .unwrap_or_else(|error| panic!("read {name}.bin: {error}"));
        let raw_json = fs::read(dir.join(format!("{name}.json")))
            .unwrap_or_else(|error| panic!("read {name}.json: {error}"));
        let json: Value = serde_json::from_slice(&raw_json)
            .unwrap_or_else(|error| panic!("parse {name}.json: {error}"));
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
    }
}

fn expected_enum_values() -> BTreeSet<String> {
    ENUM_VALUES
        .iter()
        .flat_map(|(enum_name, values)| {
            values
                .iter()
                .map(move |value| format!("{enum_name}::{value}"))
        })
        .collect()
}

fn documented_enum_values(dir: &Path) -> BTreeSet<String> {
    let mut values = BTreeSet::new();
    for entry in fs::read_dir(dir).expect("vector directory exists") {
        let path = entry.expect("vector directory entry").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let json: Value = serde_json::from_slice(
            &fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
        )
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
        for value in json["non_default_enums"]
            .as_array()
            .unwrap_or_else(|| panic!("{} has no non_default_enums array", path.display()))
        {
            values.insert(
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("{} has a non-string enum entry", path.display()))
                    .to_owned(),
            );
        }
    }
    values
}

fn parse_enum_value(value: &str) -> Result<(), ()> {
    let (enum_name, name) = value.split_once("::").ok_or(())?;
    let parsed = match enum_name {
        "FileError" => pb::FileError::from_str_name(name).is_some(),
        "FileEntry.Kind" => pb::file_entry::Kind::from_str_name(name).is_some(),
        "FileStatus.State" => pb::file_status::State::from_str_name(name).is_some(),
        "Platform" => pb::Platform::from_str_name(name).is_some(),
        "Intent" => pb::Intent::from_str_name(name).is_some(),
        "PathKind" => pb::PathKind::from_str_name(name).is_some(),
        "Transport" => pb::Transport::from_str_name(name).is_some(),
        "Codec" => pb::Codec::from_str_name(name).is_some(),
        "Role" => pb::Role::from_str_name(name).is_some(),
        "Quality" => pb::Quality::from_str_name(&format!("QUALITY_{name}")).is_some(),
        "AuthResult.Status" => pb::auth_result::Status::from_str_name(name).is_some(),
        "SessionAction.Kind" => pb::session_action::Kind::from_str_name(name).is_some(),
        "KeyframeRequest.Reason" => pb::keyframe_request::Reason::from_str_name(name).is_some(),
        "PairResult.Status" => pb::pair_result::Status::from_str_name(name).is_some(),
        "Goodbye.Reason" => pb::goodbye::Reason::from_str_name(name).is_some(),
        "ViewportAck.Mode" => pb::viewport_ack::Mode::from_str_name(name).is_some(),
        "VirtualDisplayResult.Status" => {
            pb::virtual_display_result::Status::from_str_name(name).is_some()
        }
        _ => false,
    };
    parsed.then_some(()).ok_or(())
}

fn count_enum_values<T>() -> usize
where
    T: TryFrom<i32>,
{
    (1..)
        .take_while(|value| T::try_from(*value).is_ok())
        .count()
}

#[test]
fn every_non_default_enum_value_has_a_vector() {
    if is_regen() {
        ensure_vector_files(&vector_dir("control_enums"), &vectors());
    }
    let expected = expected_enum_values();
    let mut actual = documented_enum_values(&vector_dir("control"));
    actual.extend(documented_enum_values(&vector_dir("control_enums")));

    let missing: Vec<_> = expected.difference(&actual).cloned().collect();
    let unexpected: Vec<_> = actual.difference(&expected).cloned().collect();
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "non-default enum vector coverage drifted; missing: [{}], unexpected: [{}]",
        missing.join(", "),
        unexpected.join(", ")
    );

    for value in &expected {
        assert!(
            parse_enum_value(value).is_ok(),
            "vector documents unknown enum value {value}"
        );
    }

    let counts = [
        ("FileError", count_enum_values::<pb::FileError>()),
        (
            "FileEntry.Kind",
            count_enum_values::<pb::file_entry::Kind>(),
        ),
        (
            "FileStatus.State",
            count_enum_values::<pb::file_status::State>(),
        ),
        ("Platform", count_enum_values::<pb::Platform>()),
        ("Intent", count_enum_values::<pb::Intent>()),
        ("PathKind", count_enum_values::<pb::PathKind>()),
        ("Transport", count_enum_values::<pb::Transport>()),
        ("Codec", count_enum_values::<pb::Codec>()),
        ("Role", count_enum_values::<pb::Role>()),
        ("Quality", count_enum_values::<pb::Quality>()),
        (
            "AuthResult.Status",
            count_enum_values::<pb::auth_result::Status>(),
        ),
        (
            "SessionAction.Kind",
            count_enum_values::<pb::session_action::Kind>(),
        ),
        (
            "KeyframeRequest.Reason",
            count_enum_values::<pb::keyframe_request::Reason>(),
        ),
        (
            "PairResult.Status",
            count_enum_values::<pb::pair_result::Status>(),
        ),
        ("Goodbye.Reason", count_enum_values::<pb::goodbye::Reason>()),
        (
            "VirtualDisplayResult.Status",
            count_enum_values::<pb::virtual_display_result::Status>(),
        ),
        (
            "ViewportAck.Mode",
            count_enum_values::<pb::viewport_ack::Mode>(),
        ),
    ];
    for (enum_name, count) in counts {
        let listed = ENUM_VALUES
            .iter()
            .find(|(name, _)| *name == enum_name)
            .map_or(0, |(_, values)| values.len());
        assert_eq!(
            count, listed,
            "prost enum {enum_name} gained or lost a value; update the hard-coded list and add a vector"
        );
    }
}

fn file_enum_vectors() -> Vec<(&'static str, pb::Envelope, Value)> {
    let mut cases = Vec::new();
    let mut hello = client_hello(
        pb::Platform::Ios,
        pb::Intent::Files,
        pb::Transport::Quic,
        "Phone",
        1280,
        720,
    );
    if let Some(pb::envelope::Msg::ClientHello(ref mut message)) = hello.msg {
        message.required_features = rsp_core::features::FILE_BROWSE;
        message.features |= rsp_core::features::FILE_BROWSE | rsp_core::features::FILE_TRANSFER;
    }
    cases.push(("client_hello_files", hello, json!({
        "name": "client_hello_files", "fields": {"intent": "FILES", "required_features": 1 << 21},
        "non_default_enums": ["Intent::FILES"], "binary": "client_hello_files.bin"
    })));
    for (name, value) in [
        ("file_errors_not_found", 1),
        ("file_errors_permission_denied", 2),
        ("file_errors_outside_root", 3),
        ("file_errors_not_a_dir", 4),
        ("file_errors_not_a_file", 5),
        ("file_errors_disabled", 6),
        ("file_errors_not_allowed", 7),
        ("file_errors_busy", 8),
        ("file_errors_io", 9),
        ("file_errors_no_space", 10),
        ("file_errors_too_large", 11),
        ("file_errors_resume_mismatch", 12),
        ("file_errors_sha256_mismatch", 13),
        ("file_errors_source_changed", 14),
        ("file_errors_protocol", 15),
    ] {
        let value = pb::FileError::try_from(value).unwrap();
        let env = pb::FileStatus {
            state: pb::file_status::State::Failed as i32,
            error: value as i32,
            ..Default::default()
        }
        .into();
        cases.push((
            name,
            env,
            json!({
                "name": name, "fields": {"value": value.as_str_name()},
                "non_default_enums": [format!("FileError::{}", value.as_str_name())],
                "binary": format!("{name}.bin")
            }),
        ));
    }
    for (name, value) in [
        ("file_states_unknown", 1),
        ("file_states_partial", 2),
        ("file_states_receiving", 3),
        ("file_states_completed", 4),
        ("file_states_failed", 5),
        ("file_states_cancelled", 6),
    ] {
        let value = pb::file_status::State::try_from(value).unwrap();
        let env = pb::FileStatus {
            state: value as i32,
            ..Default::default()
        }
        .into();
        cases.push((
            name,
            env,
            json!({
                "name": name, "fields": {"value": value.as_str_name()},
                "non_default_enums": [format!("FileStatus.State::{}", value.as_str_name())],
                "binary": format!("{name}.bin")
            }),
        ));
    }
    for (name, value) in [
        ("file_kinds_file", 1),
        ("file_kinds_dir", 2),
        ("file_kinds_symlink", 3),
        ("file_kinds_other", 4),
    ] {
        let value = pb::file_entry::Kind::try_from(value).unwrap();
        let env = pb::FileListing {
            entries: vec![pb::FileEntry {
                name: "entry".into(),
                kind: value as i32,
                child_count: -1,
                ..Default::default()
            }],
            ..Default::default()
        }
        .into();
        cases.push((
            name,
            env,
            json!({
                "name": name, "fields": {"value": value.as_str_name()},
                "non_default_enums": [format!("FileEntry.Kind::{}", value.as_str_name())],
                "binary": format!("{name}.bin")
            }),
        ));
    }
    cases
}
