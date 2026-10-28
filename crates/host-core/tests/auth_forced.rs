use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use host_core::auth::{
    decide, parse_fail_code, AuthDecision, AuthInput, AuthPolicy, DeviceStatus, PathClass,
    SessionSnapshot, WhoIsOutcome,
};
use rsp_core::errors::ErrorCode;
use rsp_core::pb::Intent;
use rsp_core::trust::Permission;

fn input(intent: Intent, device: DeviceStatus) -> AuthInput {
    AuthInput {
        intent,
        local_addr: addr(7400),
        peer_addr: addr(1234),
        path_class: PathClass::Lan,
        device,
        whois: WhoIsOutcome::NotApplicable,
        policy: AuthPolicy::default(),
        trust_stale_24h: false,
        session: SessionSnapshot::default(),
        local_activity_within_60s: false,
    }
}

fn addr(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

fn denied_code(decision: AuthDecision) -> ErrorCode {
    match decision {
        AuthDecision::Denied { code, .. } => code,
        other => panic!("expected denial, got {other:?}"),
    }
}

#[test]
fn forced_denial_precedes_normal_admission() {
    let mut value = input(Intent::Control, DeviceStatus::paired(Permission::Control));
    value.policy.forced_denial = Some(ErrorCode::SessionFull);
    assert!(matches!(
        decide(&value),
        AuthDecision::Denied {
            code: ErrorCode::SessionFull,
            log: "forced by --fail",
        }
    ));

    value.device = DeviceStatus::Unknown;
    assert_eq!(denied_code(decide(&value)), ErrorCode::SessionFull);
}

#[test]
fn probe_ignores_forced_denial() {
    let mut value = input(Intent::Probe, DeviceStatus::Revoked);
    value.policy.forced_denial = Some(ErrorCode::SessionFull);
    assert_eq!(decide(&value), AuthDecision::Probe);
}

#[test]
fn no_forced_denial_preserves_existing_decisions() {
    let mut value = input(Intent::Control, DeviceStatus::Unknown);
    assert_eq!(denied_code(decide(&value)), ErrorCode::NotPaired);

    value.device = DeviceStatus::Revoked;
    assert_eq!(denied_code(decide(&value)), ErrorCode::DeviceRevoked);
}

#[test]
fn parses_supported_fail_codes() {
    for (text, expected) in [
        ("RS-E142", ErrorCode::SessionFull),
        ("142", ErrorCode::SessionFull),
        (" rs-e142 ", ErrorCode::SessionFull),
        ("170", ErrorCode::HostTooOld),
        ("180", ErrorCode::DeviceRevoked),
        ("190", ErrorCode::LoginWindow),
    ] {
        assert_eq!(parse_fail_code(text), Ok(expected), "input {text:?}");
    }
}

#[test]
fn rejects_unknown_or_malformed_fail_codes() {
    for text in ["RS-E999", "", "abc"] {
        let error = parse_fail_code(text).expect_err(text);
        assert!(error.contains(&format!("{text:?}")), "{text:?}: {error}");
    }
}
