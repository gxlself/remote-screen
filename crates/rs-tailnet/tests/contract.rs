use rs_tailnet::{
    abi::{code_to_io_kind, ABI_VERSION, SYMBOLS},
    BackendState, Code, TailnetConfig, TailnetEvent, TailnetStatus,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::ErrorKind,
};

#[test]
fn error_code_table() {
    for (raw, code, kind) in [
        (0, Code::Ok, ErrorKind::Other),
        (-1, Code::Invalid, ErrorKind::InvalidInput),
        (-2, Code::State, ErrorKind::Other),
        (-3, Code::NotRunning, ErrorKind::NotConnected),
        (-4, Code::Timeout, ErrorKind::TimedOut),
        (-5, Code::Refused, ErrorKind::ConnectionRefused),
        (-6, Code::Unreachable, ErrorKind::HostUnreachable),
        (-7, Code::Io, ErrorKind::Other),
        (-8, Code::Closed, ErrorKind::BrokenPipe),
        (-99, Code::Unknown, ErrorKind::Other),
    ] {
        assert_eq!(Code::from_raw(raw), code);
        assert_eq!(code_to_io_kind(code), kind);
    }
    assert_eq!(Code::from_raw(i64::MAX), Code::Unknown);
    assert_eq!(Code::from_raw(i64::MIN), Code::Unknown);
}

#[test]
fn c_header_matches_abi() {
    let header = include_str!("../../../tailnet/include/rs_tailnet.h");
    let definitions: BTreeMap<_, _> = header
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_whitespace();
            if tokens.next()? != "#define" {
                return None;
            }
            Some((tokens.next()?, tokens.next()?.parse::<i32>().ok()?))
        })
        .collect();
    assert_eq!(definitions["RS_TS_ABI_VERSION"], ABI_VERSION);
    assert_eq!(definitions["RS_TS_OK"], Code::Ok as i32);
    let errors = [
        ("RS_TS_ERR_INVALID", Code::Invalid),
        ("RS_TS_ERR_STATE", Code::State),
        ("RS_TS_ERR_NOT_RUNNING", Code::NotRunning),
        ("RS_TS_ERR_TIMEOUT", Code::Timeout),
        ("RS_TS_ERR_REFUSED", Code::Refused),
        ("RS_TS_ERR_UNREACHABLE", Code::Unreachable),
        ("RS_TS_ERR_IO", Code::Io),
        ("RS_TS_ERR_CLOSED", Code::Closed),
    ];
    assert_eq!(
        definitions
            .keys()
            .filter(|key| key.starts_with("RS_TS_ERR_"))
            .count(),
        errors.len()
    );
    for (name, code) in errors {
        assert_eq!(definitions[name], code as i32, "{name}");
    }
    let functions: Vec<_> = header
        .lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once("rs_ts_")?;
            let (name, _) = rest.split_once('(')?;
            Some(format!("rs_ts_{name}"))
        })
        .collect();
    assert_eq!(
        functions
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        SYMBOLS.into_iter().collect()
    );
    assert_eq!(functions, SYMBOLS);
}

#[test]
fn config_keys_and_secrets() {
    let mut cfg = TailnetConfig {
        dir: "example-state".into(),
        hostname: "rs-app".into(),
        control_url: "https://hs.example.com".into(),
        auth_key: Some("tskey-contract-secret".into()),
        ephemeral: true,
        log_file: Some("example.log".into()),
        verbose: true,
        disable_netns: true,
        udp_idle_seconds: 15,
    };
    let value = serde_json::to_value(&cfg).unwrap();
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        "dir hostname controlUrl authKey ephemeral logFile verbose disableNetns udpIdleSeconds"
            .split_whitespace()
            .collect()
    );
    assert_eq!(value["authKey"], "tskey-contract-secret");
    let debug = format!("{cfg:?}");
    assert!(!debug.contains("tskey-contract-secret"));
    assert!(debug.contains("Some(\"<redacted>\")"));
    cfg.auth_key = None;
    cfg.log_file = None;
    let value = serde_json::to_value(cfg).unwrap();
    assert!(value.get("authKey").is_none());
    assert!(value.get("logFile").is_none());
    let defaults = serde_json::to_value(TailnetConfig::default()).unwrap();
    assert_eq!(defaults["dir"], "");
    assert_eq!(defaults["udpIdleSeconds"], 0);
}

#[test]
fn status_fixture_round_trips() {
    let json = include_str!("../../../tailnet/testdata/status.json");
    let status: TailnetStatus = serde_json::from_str(json).unwrap();
    assert_eq!(status.state, BackendState::Running);
    assert!(!status.self_node.as_ref().unwrap().ips.is_empty());
    assert_eq!(
        status.peers[0].direct,
        serde_json::from_str::<serde_json::Value>(json).unwrap()["peers"][0]["direct"]
            .as_bool()
            .unwrap()
    );
    let round_trip =
        serde_json::from_slice::<TailnetStatus>(&serde_json::to_vec(&status).unwrap()).unwrap();
    assert_eq!(round_trip, status);
    let minimal: TailnetStatus =
        serde_json::from_str(r#"{"future":1,"self":{},"peers":[{}]}"#).unwrap();
    assert_eq!(minimal.state, BackendState::NoState);
    assert_eq!(minimal.peers[0].last_handshake_us, 0);
}

#[test]
fn event_fixtures_and_forward_compatibility() {
    let events: Vec<TailnetEvent> = include_str!("../../../tailnet/testdata/events.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let mut variants = BTreeSet::new();
    for event in events {
        variants.insert(match event {
            TailnetEvent::State { .. } => "state",
            TailnetEvent::AuthUrl { .. } => "authUrl",
            TailnetEvent::Netmap => "netmap",
            TailnetEvent::Error { .. } => "error",
            TailnetEvent::Stopped => "stopped",
            TailnetEvent::Unknown => panic!("unknown fixture event"),
        });
        assert_eq!(
            serde_json::from_slice::<TailnetEvent>(&serde_json::to_vec(&event).unwrap()).unwrap(),
            event
        );
    }
    assert_eq!(variants.len(), 5);
    assert_eq!(
        serde_json::from_str::<TailnetEvent>(r#"{"type":"future"}"#).unwrap(),
        TailnetEvent::Unknown
    );
    assert_eq!(
        serde_json::from_str::<TailnetEvent>(r#"{"type":"state","state":"Weird","authUrl":null}"#)
            .unwrap(),
        TailnetEvent::State {
            state: BackendState::Unknown,
            auth_url: None
        }
    );
    for (state, camel) in [
        (BackendState::NoState, "noState"),
        (BackendState::NeedsLogin, "needsLogin"),
        (BackendState::NeedsMachineAuth, "needsMachineAuth"),
        (BackendState::Stopped, "stopped"),
        (BackendState::Starting, "starting"),
        (BackendState::Running, "running"),
        (BackendState::Unknown, "unknown"),
    ] {
        assert_eq!(state.as_camel(), camel);
    }
}
