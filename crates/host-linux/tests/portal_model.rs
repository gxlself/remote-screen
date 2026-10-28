use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use host_linux::portal_model::{
    cursor, device, plan, request_path, session_path, source, stream_displays, valid_token, Call,
    FlowError, FlowState, Method, OptValue, PortalCaps, PortalFlow, PortalKind, PortalPlan, RVal,
    RestoreTokenStore, StreamInfo,
};

const GNOME: PortalCaps = PortalCaps {
    rd_version: Some(2),
    rd_devices: 7,
    sc_version: Some(5),
    sc_sources: 7,
    sc_cursor_modes: 7,
};
const WLR: PortalCaps = PortalCaps {
    rd_version: None,
    rd_devices: 0,
    sc_version: Some(4),
    sc_sources: 3,
    sc_cursor_modes: 3,
};
const OLD: PortalCaps = PortalCaps {
    rd_version: Some(1),
    rd_devices: 3,
    sc_version: Some(1),
    sc_sources: 1,
    sc_cursor_modes: 0,
};

fn response(name: &str, value: RVal) -> Vec<(String, RVal)> {
    vec![(name.to_owned(), value)]
}

fn option(name: &'static str, value: OptValue) -> (&'static str, OptValue) {
    (name, value)
}

#[test]
fn plan_variants() {
    assert_eq!(
        plan(&GNOME, true).expect("GNOME plan"),
        PortalPlan {
            kind: PortalKind::RemoteDesktop,
            devices: device::KEYBOARD | device::POINTER,
            cursor_mode: cursor::METADATA,
            persist: true,
        }
    );
    assert_eq!(
        plan(&GNOME, false).expect("GNOME screencast plan"),
        PortalPlan {
            kind: PortalKind::ScreenCast,
            devices: 0,
            cursor_mode: cursor::METADATA,
            persist: true,
        }
    );
    assert_eq!(
        plan(&WLR, true).expect("WLR plan"),
        PortalPlan {
            kind: PortalKind::ScreenCast,
            devices: 0,
            cursor_mode: cursor::EMBEDDED,
            persist: true,
        }
    );
    assert_eq!(
        plan(&OLD, true).expect("old plan"),
        PortalPlan {
            kind: PortalKind::RemoteDesktop,
            devices: 3,
            cursor_mode: 0,
            persist: false,
        }
    );
    assert_eq!(
        plan(
            &PortalCaps {
                sc_version: None,
                ..PortalCaps::default()
            },
            false,
        )
        .unwrap_err()
        .to_string(),
        "no ScreenCast portal"
    );
    assert_eq!(
        plan(
            &PortalCaps {
                sc_version: Some(1),
                sc_sources: source::WINDOW,
                ..PortalCaps::default()
            },
            false,
        )
        .unwrap_err()
        .to_string(),
        "portal cannot share monitors"
    );
}

#[test]
fn rd_flow_calls() {
    let plan = plan(&GNOME, true).expect("plan");
    let mut flow = PortalFlow::new(plan, ":1.42", "rs7_", Some("tok".to_owned()));

    let first = flow.next_call().expect("create call");
    assert_eq!(
        first,
        Call {
            method: Method::RdCreateSession,
            session: None,
            options: vec![
                option("handle_token", OptValue::Str("rs7_0".to_owned())),
                option("session_handle_token", OptValue::Str("rs7_s".to_owned()),),
            ],
            request_path: "/org/freedesktop/portal/desktop/request/1_42/rs7_0".to_owned(),
        }
    );
    assert_eq!(
        flow.on_response(
            0,
            &response(
                "session_handle",
                RVal::Str("/org/freedesktop/portal/desktop/session/1_42/rs7_s".to_owned()),
            ),
        ),
        Ok(FlowState::Continue)
    );

    let devices = flow.next_call().expect("select devices call");
    assert_eq!(devices.method, Method::SelectDevices);
    assert_eq!(
        devices.session.as_deref(),
        Some("/org/freedesktop/portal/desktop/session/1_42/rs7_s")
    );
    assert_eq!(
        devices.options,
        vec![
            option("handle_token", OptValue::Str("rs7_1".to_owned())),
            option("types", OptValue::U32(3)),
            option("persist_mode", OptValue::U32(2)),
            option("restore_token", OptValue::Str("tok".to_owned())),
        ]
    );
    assert_eq!(flow.on_response(0, &[]), Ok(FlowState::Continue));

    let sources = flow.next_call().expect("select sources call");
    assert_eq!(sources.method, Method::SelectSources);
    assert_eq!(
        sources.options,
        vec![
            option("handle_token", OptValue::Str("rs7_2".to_owned())),
            option("types", OptValue::U32(source::MONITOR)),
            option("multiple", OptValue::Bool(true)),
            option("cursor_mode", OptValue::U32(cursor::METADATA)),
        ]
    );
    assert_eq!(flow.on_response(0, &[]), Ok(FlowState::Continue));

    let start = flow.next_call().expect("start call");
    assert_eq!(start.method, Method::RdStart);
    assert_eq!(
        start.options,
        vec![option("handle_token", OptValue::Str("rs7_3".to_owned()))]
    );
    assert_eq!(start.session, devices.session);
}

#[test]
fn sc_flow_persist_in_select_sources() {
    let mut flow = PortalFlow::new(
        plan(&WLR, false).expect("plan"),
        ":1.42",
        "rs_",
        Some("t".to_owned()),
    );
    let _ = flow.next_call().expect("create");
    assert_eq!(
        flow.on_response(
            0,
            &response("session_handle", RVal::Str("session".to_owned()))
        ),
        Ok(FlowState::Continue)
    );
    let sources = flow.next_call().expect("sources");
    assert_eq!(sources.method, Method::SelectSources);
    assert_eq!(
        sources.options,
        vec![
            option("handle_token", OptValue::Str("rs_1".to_owned())),
            option("types", OptValue::U32(source::MONITOR)),
            option("multiple", OptValue::Bool(true)),
            option("cursor_mode", OptValue::U32(cursor::EMBEDDED)),
            option("persist_mode", OptValue::U32(2)),
            option("restore_token", OptValue::Str("t".to_owned())),
        ]
    );
}

#[test]
fn no_restore_token_when_not_persisting() {
    let mut flow = PortalFlow::new(
        plan(&OLD, true).expect("plan"),
        ":1.42",
        "old_",
        Some("t".to_owned()),
    );
    let create = flow.next_call().expect("create");
    assert!(!create
        .options
        .iter()
        .any(|(key, _)| *key == "restore_token" || *key == "persist_mode"));
    assert_eq!(
        flow.on_response(
            0,
            &response("session_handle", RVal::Str("session".to_owned()))
        ),
        Ok(FlowState::Continue)
    );
    let devices = flow.next_call().expect("devices");
    assert!(devices
        .options
        .iter()
        .all(|(key, _)| *key != "restore_token" && *key != "persist_mode"));
    assert_eq!(flow.on_response(0, &[]), Ok(FlowState::Continue));
    let sources = flow.next_call().expect("sources");
    assert!(sources
        .options
        .iter()
        .all(|(key, _)| *key != "restore_token"
            && *key != "persist_mode"
            && *key != "cursor_mode"));
}

#[test]
fn next_call_waits_for_response() {
    let mut flow = PortalFlow::new(plan(&GNOME, true).expect("plan"), ":1.42", "rs_", None);
    assert!(flow.next_call().is_some());
    assert!(flow.next_call().is_none());
    assert_eq!(
        flow.on_response(
            0,
            &response("session_handle", RVal::Str("session".to_owned()))
        ),
        Ok(FlowState::Continue)
    );
    assert_eq!(
        flow.next_call().map(|call| call.method),
        Some(Method::SelectDevices)
    );
}

#[test]
fn session_handle_accepts_str() {
    let mut flow = PortalFlow::new(plan(&GNOME, true).expect("plan"), ":1.42", "rs7_", None);
    let _ = flow.next_call();
    assert_eq!(
        flow.on_response(
            0,
            &response(
                "session_handle",
                RVal::Str("/org/freedesktop/portal/desktop/session/1_42/rs7_s".to_owned()),
            ),
        ),
        Ok(FlowState::Continue)
    );
    assert_eq!(
        flow.next_call().and_then(|call| call.session),
        Some("/org/freedesktop/portal/desktop/session/1_42/rs7_s".to_owned())
    );
}

#[test]
fn errors() {
    let mut cancelled = PortalFlow::new(plan(&GNOME, true).expect("plan"), ":1", "rs_", None);
    let _ = cancelled.next_call();
    assert_eq!(cancelled.on_response(1, &[]), Err(FlowError::Cancelled));
    assert!(cancelled.next_call().is_none());

    let mut failed = PortalFlow::new(plan(&GNOME, true).expect("plan"), ":1", "rs_", None);
    let _ = failed.next_call();
    assert_eq!(failed.on_response(2, &[]), Err(FlowError::Failed(2)));

    let mut missing = PortalFlow::new(plan(&GNOME, true).expect("plan"), ":1", "rs_", None);
    let _ = missing.next_call();
    assert_eq!(missing.on_response(0, &[]), Err(FlowError::MissingSession));

    let mut no_streams = PortalFlow::new(plan(&GNOME, true).expect("plan"), ":1", "rs_", None);
    let _ = no_streams.next_call();
    let _ = no_streams.on_response(0, &response("session_handle", RVal::Str("s".to_owned())));
    let _ = no_streams.next_call();
    let _ = no_streams.on_response(0, &[]);
    let _ = no_streams.next_call();
    let _ = no_streams.on_response(0, &[]);
    let _ = no_streams.next_call();
    assert_eq!(no_streams.on_response(0, &[]), Err(FlowError::NoStreams));
    assert_eq!(no_streams.on_response(0, &[]), Err(FlowError::Unexpected));
}

#[test]
fn start_results() {
    let mut flow = PortalFlow::new(plan(&GNOME, true).expect("plan"), ":1", "rs_", None);
    let _ = flow.next_call();
    let _ = flow.on_response(0, &response("session_handle", RVal::Str("s".to_owned())));
    let _ = flow.next_call();
    let _ = flow.on_response(0, &[]);
    let _ = flow.next_call();
    let _ = flow.on_response(0, &[]);
    let _ = flow.next_call();
    let streams = vec![StreamInfo::default(), StreamInfo::default()];
    let result = flow
        .on_response(
            0,
            &[
                ("devices".to_owned(), RVal::U32(3)),
                ("streams".to_owned(), RVal::Streams(streams.clone())),
                ("restore_token".to_owned(), RVal::Str("new".to_owned())),
            ],
        )
        .expect("start response");
    assert_eq!(
        result,
        FlowState::Done(host_linux::portal_model::StartResults {
            kind: PortalKind::RemoteDesktop,
            session: "s".to_owned(),
            devices: 3,
            streams,
            restore_token: Some("new".to_owned()),
            clipboard_enabled: false,
        })
    );
}

#[test]
fn paths_and_tokens() {
    assert_eq!(
        request_path(":1.42", "a"),
        "/org/freedesktop/portal/desktop/request/1_42/a"
    );
    assert_eq!(
        session_path(":1.42", "a"),
        "/org/freedesktop/portal/desktop/session/1_42/a"
    );
    assert!(valid_token("rs1_0"));
    assert!(!valid_token(""));
    assert!(!valid_token("a-b"));
    assert!(!valid_token("a.b"));
}

fn stream(node_id: u32, position: Option<(i32, i32)>, size: Option<(i32, i32)>) -> StreamInfo {
    StreamInfo {
        node_id,
        position,
        size,
        source_type: Some(source::MONITOR),
        ..StreamInfo::default()
    }
}

#[test]
fn stream_layout() {
    let displays = stream_displays(&[
        stream(1, Some((100, 20)), Some((1920, 1080))),
        stream(2, Some((0, 0)), Some((1280, 1024))),
    ]);
    assert_eq!(displays[0].x, 100);
    assert!(!displays[0].primary);
    assert_eq!(displays[1].x, 0);
    assert!(displays[1].primary);

    let displays = stream_displays(&[
        stream(1, None, Some((1920, 1080))),
        stream(2, None, Some((1280, 1024))),
        stream(3, None, None),
    ]);
    assert_eq!((displays[0].x, displays[0].y), (0, 0));
    assert_eq!((displays[1].x, displays[1].y), (1920, 0));
    assert_eq!((displays[2].x, displays[2].y), (3200, 0));
    assert_eq!((displays[2].width, displays[2].height), (0, 0));

    let displays = stream_displays(&[
        stream(1, Some((0, 0)), Some((10, 10))),
        StreamInfo {
            node_id: 2,
            source_type: Some(source::WINDOW),
            ..StreamInfo::default()
        },
    ]);
    assert_eq!(displays.len(), 1);
    assert_eq!(displays[0].id, 1);
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_state_dir() -> PathBuf {
    let number = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "remote-screen-portal-test-{}-{number}",
        std::process::id()
    ))
}

#[test]
fn restore_store_roundtrip() {
    let dir = temp_state_dir();
    let store = RestoreTokenStore::new(&dir);
    store
        .save(PortalKind::RemoteDesktop, "abc")
        .expect("save token");
    assert_eq!(
        store.load(PortalKind::RemoteDesktop),
        Some("abc".to_owned())
    );
    assert_eq!(store.load(PortalKind::ScreenCast), None);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(dir.join("portal-restore.json"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&dir)
                .expect("dir metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    store.clear().expect("clear token");
    assert_eq!(store.load(PortalKind::RemoteDesktop), None);
    store.clear().expect("clear missing token");
    fs::remove_dir_all(dir).expect("remove test directory");
}

#[test]
fn restore_store_rejects_garbage() {
    let dir = temp_state_dir();
    fs::create_dir_all(&dir).expect("create test directory");
    let path = dir.join("portal-restore.json");
    fs::write(&path, b"not json").expect("write garbage");
    let store = RestoreTokenStore::new(&dir);
    assert_eq!(store.load(PortalKind::RemoteDesktop), None);
    fs::write(
        &path,
        br#"{"version":2,"kind":"remote_desktop","token":"abc"}"#,
    )
    .expect("write old version");
    assert_eq!(store.load(PortalKind::RemoteDesktop), None);
    fs::remove_dir_all(dir).expect("remove test directory");
}
