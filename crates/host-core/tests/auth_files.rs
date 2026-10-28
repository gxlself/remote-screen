use host_core::auth::{
    decide, ApprovalReason, AuthDecision, AuthInput, AuthPolicy, AuthWarning, DeviceStatus,
    PathClass, PathPermission, Role, SessionSnapshot, WhoIsOutcome,
};
use rsp_core::{errors::ErrorCode, pb::Intent, trust::Permission};

fn input() -> AuthInput {
    AuthInput {
        intent: Intent::Files,
        local_addr: "100.64.0.1:0".parse().unwrap(),
        peer_addr: "100.64.0.2:0".parse().unwrap(),
        path_class: PathClass::Tailnet,
        device: DeviceStatus::paired(Permission::Control),
        whois: WhoIsOutcome::Ok {
            login: "alice@example.com".into(),
            stable_id: "node-a".into(),
            node_name: "phone".into(),
        },
        policy: AuthPolicy {
            allowed_users: vec!["alice@example.com".into()],
            ..Default::default()
        },
        trust_stale_24h: false,
        session: SessionSnapshot::default(),
        local_activity_within_60s: false,
    }
}

fn accepted(role: Role, reasons: &[ApprovalReason]) -> AuthDecision {
    AuthDecision::Accept {
        role,
        needs_approval: (!reasons.is_empty()).then(|| reasons.to_vec()),
        warnings: vec![],
    }
}

fn assert_denied(value: &AuthInput, expected: ErrorCode) {
    assert!(matches!(decide(value), AuthDecision::Denied { code, .. } if code == expected));
}

#[test]
fn paired_control_device_gets_viewer_without_approval() {
    assert_eq!(decide(&input()), accepted(Role::Viewer, &[]));
}

#[test]
fn files_skip_viewer_capacity_even_when_no_viewers_are_allowed() {
    let mut value = input();
    value.session.has_controller = true;
    value.session.viewer_count = value.policy.max_viewers;
    assert_eq!(decide(&value), accepted(Role::Viewer, &[]));
    value.policy.max_viewers = 0;
    assert_eq!(decide(&value), accepted(Role::Viewer, &[]));
}

#[test]
fn view_only_device_is_denied_even_on_control_path() {
    let mut value = input();
    value.device = DeviceStatus::paired_with_paths(
        Permission::ViewOnly,
        PathPermission::Control,
        PathPermission::Control,
        None,
        None,
    );
    for path in [PathClass::Tailnet, PathClass::Intranet, PathClass::Lan] {
        value.path_class = path;
        assert_eq!(
            decide(&value),
            AuthDecision::Denied {
                code: ErrorCode::PolicyDenied,
                log: "view-only device cannot transfer files",
            }
        );
    }
}

#[test]
fn view_only_path_and_whois_fallback_are_denied() {
    let mut value = input();
    value.device = DeviceStatus::paired_with_paths(
        Permission::Control,
        PathPermission::ViewOnly,
        PathPermission::ViewOnly,
        None,
        None,
    );
    for path in [PathClass::Intranet, PathClass::Lan] {
        value.path_class = path;
        assert_denied(&value, ErrorCode::PolicyDenied);
    }
    value.path_class = PathClass::Tailnet;
    value.whois = WhoIsOutcome::Unavailable;
    assert_denied(&value, ErrorCode::PolicyDenied);
}

#[test]
fn files_preserve_source_identity_and_path_restrictions() {
    let mut value = input();
    value.path_class = PathClass::Unknown;
    assert_denied(&value, ErrorCode::PolicyDenied);
    value.path_class = PathClass::Tailnet;
    value.policy.allowed_users.clear();
    assert_denied(&value, ErrorCode::PolicyDenied);
    value.device = DeviceStatus::paired_with_paths(
        Permission::Control,
        PathPermission::Deny,
        PathPermission::Deny,
        None,
        None,
    );
    for path in [PathClass::Intranet, PathClass::Lan] {
        value.path_class = path;
        assert_denied(&value, ErrorCode::IntranetDenied);
    }
    value.path_class = PathClass::Tailnet;
    value.whois = WhoIsOutcome::Unavailable;
    assert_denied(&value, ErrorCode::IntranetDenied);
}

#[test]
fn host_switch_pairing_and_revocation_still_apply() {
    let mut value = input();
    value.policy.allow_remote_control = false;
    assert_denied(&value, ErrorCode::RemoteControlDisabled);
    value.device = DeviceStatus::Unknown;
    assert_denied(&value, ErrorCode::NotPaired);
    value.device = DeviceStatus::Revoked;
    assert_denied(&value, ErrorCode::DeviceRevoked);
}

#[test]
fn local_activity_does_not_require_approval_but_attended_mode_does() {
    let mut value = input();
    value.local_activity_within_60s = true;
    assert_eq!(decide(&value), accepted(Role::Viewer, &[]));
    value.policy.attended_mode = true;
    assert_eq!(
        decide(&value),
        accepted(Role::Viewer, &[ApprovalReason::AttendedMode])
    );
}

#[test]
fn other_approval_reasons_are_preserved_in_order() {
    let mut value = input();
    value.local_activity_within_60s = true;
    value.policy.attended_mode = true;
    value.policy.intranet_confirm_each = true;
    value.path_class = PathClass::Intranet;
    if let DeviceStatus::Paired {
        require_confirm, ..
    } = &mut value.device
    {
        *require_confirm = true;
    }
    assert_eq!(
        decide(&value),
        accepted(
            Role::Viewer,
            &[
                ApprovalReason::AttendedMode,
                ApprovalReason::IntranetConfirmation,
                ApprovalReason::DeviceRequiresConfirmation,
            ]
        )
    );
    value.policy.intranet_confirm_each = false;
    value.trust_stale_24h = true;
    assert_eq!(
        decide(&value),
        accepted(
            Role::Viewer,
            &[
                ApprovalReason::AttendedMode,
                ApprovalReason::IntranetConfirmation,
                ApprovalReason::DeviceRequiresConfirmation,
            ]
        )
    );
}

#[test]
fn node_mismatch_still_warns_and_requires_approval() {
    let mut value = input();
    value.device = DeviceStatus::paired_with_paths(
        Permission::Control,
        PathPermission::Control,
        PathPermission::Control,
        None,
        Some("another-node".into()),
    );
    assert_eq!(
        decide(&value),
        AuthDecision::Accept {
            role: Role::Viewer,
            needs_approval: Some(vec![ApprovalReason::NodeMismatch]),
            warnings: vec![AuthWarning::NodeMismatch],
        }
    );
}

#[test]
fn unattended_device_also_remains_a_files_viewer() {
    let mut value = input();
    value.device = DeviceStatus::paired(Permission::Unattended);
    assert_eq!(decide(&value), accepted(Role::Viewer, &[]));
}

#[test]
fn control_and_view_keep_existing_roles_capacity_and_local_approval() {
    for intent in [Intent::Control, Intent::View] {
        let mut value = input();
        value.intent = intent;
        let role = if intent == Intent::Control {
            Role::Controller
        } else {
            Role::Viewer
        };
        assert_eq!(decide(&value), accepted(role, &[]));
        value.local_activity_within_60s = true;
        assert_eq!(
            decide(&value),
            accepted(role, &[ApprovalReason::LocalActivity])
        );
        value.session.has_controller = true;
        assert_eq!(
            decide(&value),
            accepted(Role::Viewer, &[ApprovalReason::LocalActivity])
        );
        value.session.viewer_count = value.policy.max_viewers;
        assert_denied(&value, ErrorCode::SessionFull);
        value.session = SessionSnapshot::default();
        value.device = DeviceStatus::paired(Permission::ViewOnly);
        assert_eq!(decide(&value), accepted(Role::Viewer, &[]));
    }
}
