use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use host_core::auth::{
    decide, ApprovalReason, AuthDecision, AuthInput, AuthPolicy, AuthWarning, DeviceStatus,
    PathClass, PathPermission, Role, SessionSnapshot, WhoIsOutcome,
};
use rsp_core::errors::ErrorCode;
use rsp_core::pb::Intent;
use rsp_core::trust::Permission;

fn input(intent: Intent, path: PathClass, device: DeviceStatus) -> AuthInput {
    AuthInput {
        intent,
        local_addr: addr(7400),
        peer_addr: addr(1234),
        path_class: path,
        device,
        whois: WhoIsOutcome::NotApplicable,
        policy: AuthPolicy {
            allowed_users: vec!["alice@".into()],
            ..AuthPolicy::default()
        },
        trust_stale_24h: false,
        session: SessionSnapshot::default(),
        local_activity_within_60s: false,
    }
}

fn addr(port: u16) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
}

fn paired(permission: Permission) -> DeviceStatus {
    DeviceStatus::Paired {
        permission,
        intranet_permission: permission.into(),
        lan_permission: permission.into(),
        paired_whois_user: None,
        paired_node_stable_id: None,
        bound_node_stable_id: None,
        require_confirm: false,
    }
}

fn code(decision: AuthDecision) -> ErrorCode {
    match decision {
        AuthDecision::Denied { code, .. } => code,
        other => panic!("expected denial, got {other:?}"),
    }
}

#[test]
fn step_01_probe_does_not_read_device_state() {
    assert_eq!(
        decide(&input(
            Intent::Probe,
            PathClass::Unknown,
            DeviceStatus::Revoked
        )),
        AuthDecision::Probe
    );
}

#[test]
fn step_02_pair_requires_intranet_opt_in() {
    let mut value = input(Intent::Pair, PathClass::Intranet, DeviceStatus::Unknown);
    assert!(matches!(
        decide(&value),
        AuthDecision::Pair { allowed: false, .. }
    ));
    value.policy.allow_intranet_pairing = true;
    assert!(matches!(
        decide(&value),
        AuthDecision::Pair { allowed: true, .. }
    ));
}

#[test]
fn step_03_listener_filtered_source_is_denied() {
    assert_eq!(
        code(decide(&input(
            Intent::Control,
            PathClass::Unknown,
            paired(Permission::Control)
        ))),
        ErrorCode::PolicyDenied
    );
}

#[test]
fn step_04_revocation_precedes_pairing() {
    assert_eq!(
        code(decide(&input(
            Intent::Control,
            PathClass::Tailnet,
            DeviceStatus::Revoked
        ))),
        ErrorCode::DeviceRevoked
    );
}

#[test]
fn step_05_unknown_device_is_denied() {
    assert_eq!(
        code(decide(&input(
            Intent::Control,
            PathClass::Tailnet,
            DeviceStatus::Unknown
        ))),
        ErrorCode::NotPaired
    );
}

#[test]
fn step_06_tailnet_user_and_node_checks() {
    let mut value = input(
        Intent::Control,
        PathClass::Tailnet,
        paired(Permission::Control),
    );
    value.whois = WhoIsOutcome::Ok {
        login: "mallory@".into(),
        stable_id: "n1".into(),
        node_name: "phone".into(),
    };
    assert_eq!(code(decide(&value)), ErrorCode::PolicyDenied);

    value.whois = WhoIsOutcome::Ok {
        login: "alice@".into(),
        stable_id: "n2".into(),
        node_name: "phone".into(),
    };
    if let DeviceStatus::Paired {
        paired_node_stable_id,
        ..
    } = &mut value.device
    {
        *paired_node_stable_id = Some("n1".into());
    }
    assert!(
        matches!(decide(&value), AuthDecision::Accept { needs_approval: Some(reasons), warnings, .. } if reasons.contains(&ApprovalReason::NodeMismatch) && warnings.contains(&AuthWarning::NodeMismatch))
    );
}

#[test]
fn step_06_whois_unavailable_uses_intranet_permission_allow_and_deny() {
    let mut value = input(
        Intent::Control,
        PathClass::Tailnet,
        DeviceStatus::Paired {
            permission: Permission::Control,
            intranet_permission: PathPermission::Control,
            lan_permission: PathPermission::Control,
            paired_whois_user: None,
            paired_node_stable_id: None,
            bound_node_stable_id: None,
            require_confirm: false,
        },
    );
    value.whois = WhoIsOutcome::Unavailable;
    assert!(
        matches!(decide(&value), AuthDecision::Accept { warnings, .. } if warnings == vec![AuthWarning::WhoIsUnavailable])
    );
    if let DeviceStatus::Paired {
        intranet_permission,
        ..
    } = &mut value.device
    {
        *intranet_permission = PathPermission::Deny;
    }
    assert_eq!(code(decide(&value)), ErrorCode::IntranetDenied);
}

#[test]
fn step_07_remote_control_switch() {
    let mut value = input(
        Intent::Control,
        PathClass::Tailnet,
        paired(Permission::Control),
    );
    value.whois = WhoIsOutcome::Ok {
        login: "alice@".into(),
        stable_id: "n".into(),
        node_name: "phone".into(),
    };
    value.policy.allow_remote_control = false;
    assert_eq!(code(decide(&value)), ErrorCode::RemoteControlDisabled);
}

#[test]
fn step_08_session_capacity() {
    let mut value = input(
        Intent::Control,
        PathClass::Intranet,
        paired(Permission::Control),
    );
    value.session = SessionSnapshot {
        has_controller: true,
        viewer_count: 3,
    };
    assert_eq!(code(decide(&value)), ErrorCode::SessionFull);
}

#[test]
fn step_09_viewer_sources() {
    let mut by_intent = input(
        Intent::View,
        PathClass::Intranet,
        paired(Permission::Control),
    );
    assert!(matches!(
        decide(&by_intent),
        AuthDecision::Accept {
            role: Role::Viewer,
            ..
        }
    ));
    by_intent.intent = Intent::Control;
    if let DeviceStatus::Paired { permission, .. } = &mut by_intent.device {
        *permission = Permission::ViewOnly;
    }
    assert!(matches!(
        decide(&by_intent),
        AuthDecision::Accept {
            role: Role::Viewer,
            ..
        }
    ));
    if let DeviceStatus::Paired { permission, .. } = &mut by_intent.device {
        *permission = Permission::Control;
    }
    by_intent.session.has_controller = true;
    assert!(matches!(
        decide(&by_intent),
        AuthDecision::Accept {
            role: Role::Viewer,
            ..
        }
    ));
}

#[test]
fn step_10_each_approval_reason() {
    type Case = (&'static str, fn(&mut AuthInput), ApprovalReason);
    let cases: [Case; 4] = [
        (
            "attended",
            |v| v.policy.attended_mode = true,
            ApprovalReason::AttendedMode,
        ),
        (
            "activity",
            |v| v.local_activity_within_60s = true,
            ApprovalReason::LocalActivity,
        ),
        (
            "intranet",
            |v| v.policy.intranet_confirm_each = true,
            ApprovalReason::IntranetConfirmation,
        ),
        (
            "stale",
            |v| v.trust_stale_24h = true,
            ApprovalReason::IntranetConfirmation,
        ),
    ];
    for (_, mutate, expected) in cases {
        let mut value = input(
            Intent::Control,
            PathClass::Intranet,
            paired(Permission::Control),
        );
        mutate(&mut value);
        assert!(
            matches!(decide(&value), AuthDecision::Accept { needs_approval: Some(reasons), .. } if reasons.contains(&expected))
        );
    }
}

#[test]
fn step_11_plain_accept_and_stale_tailnet_has_no_approval() {
    let mut value = input(
        Intent::Control,
        PathClass::Tailnet,
        paired(Permission::Unattended),
    );
    value.whois = WhoIsOutcome::Ok {
        login: "alice@".into(),
        stable_id: "n".into(),
        node_name: "phone".into(),
    };
    value.trust_stale_24h = true;
    assert!(matches!(
        decide(&value),
        AuthDecision::Accept {
            role: Role::Controller,
            needs_approval: None,
            ..
        }
    ));
}

fn stored_pairing(name: &str) -> (std::path::PathBuf, host_core::store::Store, [u8; 32]) {
    use host_core::store::{DevicePlatform, PairedDevice, PairingPath, PathPermissions, Store};
    let root = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target"));
    let path = root.join(format!("rsp-auth-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    let store = Store::open(&path).unwrap();
    let fp = [71; 32];
    store
        .upsert(PairedDevice {
            fp,
            device_id: rsp_core::ids::id_from_fp(&fp),
            name: "phone".into(),
            cert_der: Vec::new(),
            permission: Permission::Unattended,
            paired_at_us: 42,
            require_confirm: false,
            platform: DevicePlatform::Ios,
            paired_via: PairingPath::Tailnet,
            whois_user: Some("alice@".into()),
            whois_node: Some("n-original".into()),
            whois_node_name: Some("phone.hs.example.com".into()),
            path_permissions: PathPermissions::default(),
        })
        .unwrap();
    (path, store, fp)
}

fn stored_auth_device(store: &host_core::store::Store, fp: &[u8; 32]) -> DeviceStatus {
    let from_status = DeviceStatus::from(store.authorize_device(fp).unwrap());
    let from_record = DeviceStatus::from(&store.get(fp).unwrap().unwrap());
    assert_eq!(from_status, from_record);
    from_status
}

#[test]
fn persisted_node_stable_id_mismatch_requires_local_approval() {
    let (path, store, fp) = stored_pairing("node-mismatch");
    let mut value = input(
        Intent::Control,
        PathClass::Tailnet,
        stored_auth_device(&store, &fp),
    );
    value.whois = WhoIsOutcome::Ok {
        login: "alice@".into(),
        stable_id: "n-replacement".into(),
        node_name: "phone.hs.example.com".into(),
    };
    assert!(matches!(decide(&value), AuthDecision::Accept {
        needs_approval: Some(reasons), warnings, ..
    } if reasons.contains(&ApprovalReason::NodeMismatch) && warnings.contains(&AuthWarning::NodeMismatch)));
    if let WhoIsOutcome::Ok {
        stable_id,
        node_name,
        ..
    } = &mut value.whois
    {
        *stable_id = "n-original".into();
        *node_name = "renamed.hs.example.com".into();
    }
    assert!(matches!(
        decide(&value),
        AuthDecision::Accept {
            needs_approval: None,
            ..
        }
    ));
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn persisted_intranet_deny_returns_error_131() {
    use host_core::store::{PairingPath, PathPermission as StoredPathPermission, Store};
    let (path, store, fp) = stored_pairing("intranet-deny");
    store
        .set_path_permission(&fp, PairingPath::Intranet, StoredPathPermission::Deny)
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    let value = input(
        Intent::Control,
        PathClass::Intranet,
        stored_auth_device(&store, &fp),
    );
    let denied = code(decide(&value));
    assert_eq!(denied, ErrorCode::IntranetDenied);
    assert_eq!(denied as u32, 131);
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn persisted_view_only_routes_limit_an_unattended_device_to_viewer() {
    use host_core::store::{PairingPath, PathPermission as StoredPathPermission};
    let (path, store, fp) = stored_pairing("view-only");
    for (route, class) in [
        (PairingPath::Intranet, PathClass::Intranet),
        (PairingPath::Lan, PathClass::Lan),
    ] {
        store
            .set_path_permission(&fp, route, StoredPathPermission::ViewOnly)
            .unwrap();
        let value = input(Intent::Control, class, stored_auth_device(&store, &fp));
        assert!(matches!(
            decide(&value),
            AuthDecision::Accept {
                role: Role::Viewer,
                ..
            }
        ));
    }
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn persisted_intranet_deny_also_applies_when_whois_is_unavailable() {
    use host_core::store::{PairingPath, PathPermission as StoredPathPermission};
    let (path, store, fp) = stored_pairing("fallback-deny");
    store
        .set_path_permission(&fp, PairingPath::Intranet, StoredPathPermission::Deny)
        .unwrap();
    let mut value = input(
        Intent::Control,
        PathClass::Tailnet,
        stored_auth_device(&store, &fp),
    );
    value.whois = WhoIsOutcome::Unavailable;
    assert_eq!(code(decide(&value)), ErrorCode::IntranetDenied);
    let _ = std::fs::remove_dir_all(path);
}
