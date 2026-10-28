//! Pure connection authentication decisions.
//!
//! The order in [`decide`] mirrors §04 "连接时的鉴权". This module deliberately
//! does not read a clock, consult the store, or perform network I/O: its caller
//! supplies a snapshot of all of those inputs.

use std::net::SocketAddr;

use rsp_core::errors::ErrorCode;
use rsp_core::pb::Intent;
use rsp_core::trust::Permission;

/// The source network selected by the listener (H11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathClass {
    Tailnet,
    Intranet,
    Lan,
    /// A source which did not pass the listener's address filters.
    Unknown,
}

impl PathClass {
    #[allow(non_upper_case_globals)]
    pub const TAILNET: Self = Self::Tailnet;
    #[allow(non_upper_case_globals)]
    pub const INTRANET: Self = Self::Intranet;
    #[allow(non_upper_case_globals)]
    pub const LAN: Self = Self::Lan;
}

/// Permission for a particular route. `Deny` is checked before the general
/// host switch; `ViewOnly` can still establish a viewer session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathPermission {
    Deny,
    ViewOnly,
    Control,
    Unattended,
}

impl From<Permission> for PathPermission {
    fn from(value: Permission) -> Self {
        match value {
            Permission::ViewOnly => Self::ViewOnly,
            Permission::Control => Self::Control,
            Permission::Unattended => Self::Unattended,
        }
    }
}

/// Device state needed by the decision algorithm.
///
/// Both store-status and pairing-record conversions preserve the persisted
/// route restrictions and WhoIs identity used by the decision algorithm.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceStatus {
    Unknown,
    Revoked,
    Paired {
        permission: Permission,
        /// Permission when the connection uses an intranet address.
        intranet_permission: PathPermission,
        /// Permission when the connection uses a LAN address.
        lan_permission: PathPermission,
        paired_whois_user: Option<String>,
        paired_node_stable_id: Option<String>,
        bound_node_stable_id: Option<String>,
        require_confirm: bool,
    },
}

impl DeviceStatus {
    pub fn paired(permission: Permission) -> Self {
        Self::Paired {
            permission,
            intranet_permission: permission.into(),
            lan_permission: permission.into(),
            paired_whois_user: None,
            paired_node_stable_id: None,
            bound_node_stable_id: None,
            require_confirm: false,
        }
    }

    pub fn paired_with_paths(
        permission: Permission,
        intranet_permission: PathPermission,
        lan_permission: PathPermission,
        paired_whois_user: Option<String>,
        paired_node_stable_id: Option<String>,
    ) -> Self {
        Self::Paired {
            permission,
            intranet_permission,
            lan_permission,
            paired_whois_user,
            paired_node_stable_id,
            bound_node_stable_id: None,
            require_confirm: false,
        }
    }
}

impl From<crate::store::DeviceStatus> for DeviceStatus {
    fn from(value: crate::store::DeviceStatus) -> Self {
        match value {
            crate::store::DeviceStatus::Unknown => Self::Unknown,
            crate::store::DeviceStatus::Revoked => Self::Revoked,
            crate::store::DeviceStatus::Paired {
                permission,
                require_confirm,
                whois_user,
                whois_node,
                bound_node,
                path_permissions,
            } => Self::Paired {
                permission,
                intranet_permission: path_permissions.intranet.into(),
                lan_permission: path_permissions.lan.into(),
                paired_whois_user: whois_user,
                paired_node_stable_id: whois_node,
                bound_node_stable_id: bound_node,
                require_confirm,
            },
        }
    }
}

impl From<crate::store::PathPermission> for PathPermission {
    fn from(value: crate::store::PathPermission) -> Self {
        match value {
            crate::store::PathPermission::Allow => Self::Control,
            crate::store::PathPermission::ViewOnly => Self::ViewOnly,
            crate::store::PathPermission::Deny => Self::Deny,
        }
    }
}

impl From<&crate::store::PairedDevice> for DeviceStatus {
    fn from(value: &crate::store::PairedDevice) -> Self {
        Self::Paired {
            permission: value.permission,
            intranet_permission: value.path_permissions.intranet.into(),
            lan_permission: value.path_permissions.lan.into(),
            paired_whois_user: value.whois_user.clone(),
            paired_node_stable_id: value.whois_node.clone(),
            bound_node_stable_id: None,
            require_confirm: value.require_confirm,
        }
    }
}

/// The policy values relevant to the eleven authentication steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthPolicy {
    pub allowed_users: Vec<String>,
    pub max_viewers: u8,
    pub allow_remote_control: bool,
    pub attended_mode: bool,
    pub intranet_confirm_each: bool,
    pub allow_intranet_pairing: bool,
    pub forced_denial: Option<ErrorCode>,
}

impl Default for AuthPolicy {
    fn default() -> Self {
        Self {
            allowed_users: Vec::new(),
            max_viewers: 3,
            allow_remote_control: true,
            attended_mode: false,
            intranet_confirm_each: false,
            allow_intranet_pairing: false,
            forced_denial: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub has_controller: bool,
    pub viewer_count: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WhoIsOutcome {
    Ok {
        login: String,
        stable_id: String,
        node_name: String,
    },
    Unavailable,
    NotApplicable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthInput {
    pub intent: Intent,
    pub local_addr: SocketAddr,
    pub peer_addr: SocketAddr,
    pub path_class: PathClass,
    pub device: DeviceStatus,
    pub whois: WhoIsOutcome,
    pub policy: AuthPolicy,
    pub trust_stale_24h: bool,
    pub session: SessionSnapshot,
    pub local_activity_within_60s: bool,
}

/// Pairing status carried by the existing `PairResult` protobuf message.
pub type PairReason = rsp_core::pb::pair_result::Status;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Controller,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalReason {
    AttendedMode,
    LocalActivity,
    IntranetConfirmation,
    NodeMismatch,
    DeviceRequiresConfirmation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthWarning {
    WhoIsUnavailable,
    NodeMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthDecision {
    Probe,
    Pair {
        allowed: bool,
        reason: PairReason,
    },
    Denied {
        code: ErrorCode,
        log: &'static str,
    },
    Accept {
        role: Role,
        needs_approval: Option<Vec<ApprovalReason>>,
        warnings: Vec<AuthWarning>,
    },
}

fn denied(code: ErrorCode, log: &'static str) -> AuthDecision {
    AuthDecision::Denied { code, log }
}

/// Parse the `--fail` argument, accepting `RS-E142` or `142` in any case.
pub fn parse_fail_code(s: &str) -> Result<ErrorCode, String> {
    let normalized = s.trim().to_ascii_lowercase();
    let digits = normalized.strip_prefix("rs-e").unwrap_or(&normalized);
    let code = digits
        .parse::<u16>()
        .map_err(|_| format!("invalid error code {s:?}"))?;
    ErrorCode::from_u16(code).ok_or_else(|| format!("invalid error code {s:?}"))
}

/// Apply the eleven connection-authentication rules in specification order.
pub fn decide(input: &AuthInput) -> AuthDecision {
    // 1. PROBE only receives the small ServerHello and is not authorized.
    if input.intent == Intent::Probe {
        return AuthDecision::Probe;
    }

    // Development-only fault injection: every authenticated intent, including
    // PAIR, returns the requested code. PROBE above remains unaffected.
    if let Some(code) = input.policy.forced_denial {
        return denied(code, "forced by --fail");
    }

    // 2. PAIR enters the pairing state machine. Only the intranet path has a
    // separate opt-in; tails and LAN are filtered by their respective layers.
    if input.intent == Intent::Pair {
        if matches!(input.path_class, PathClass::Intranet) && !input.policy.allow_intranet_pairing {
            return AuthDecision::Pair {
                allowed: false,
                reason: PairReason::DisabledOnPath,
            };
        }
        if matches!(input.path_class, PathClass::Unknown) {
            return AuthDecision::Pair {
                allowed: false,
                reason: PairReason::Denied,
            };
        }
        return AuthDecision::Pair {
            allowed: true,
            reason: PairReason::Ok,
        };
    }

    // 3. Listener source filtering is represented by PathClass::Unknown here.
    if matches!(input.path_class, PathClass::Unknown) {
        return denied(ErrorCode::PolicyDenied, "source path was not allowed");
    }

    // 4. Revocation wins over every other device state.
    if matches!(input.device, DeviceStatus::Revoked) {
        return denied(ErrorCode::DeviceRevoked, "device is revoked");
    }

    // 5. The certificate is not in the pairing table.
    let DeviceStatus::Paired {
        permission,
        intranet_permission,
        lan_permission,
        paired_whois_user,
        paired_node_stable_id,
        bound_node_stable_id,
        require_confirm,
    } = &input.device
    else {
        return denied(ErrorCode::NotPaired, "device is not paired");
    };

    // 6. Tailnet identity is checked against WhoIs. If tailscaled is down,
    // continue with the intranet permission exactly as §04 requires.
    let mut warnings = Vec::new();
    let mut node_mismatch = false;
    let mut effective_path = input.path_class;
    let mut effective_permission = match input.path_class {
        PathClass::Tailnet => PathPermission::from(*permission),
        PathClass::Intranet => *intranet_permission,
        PathClass::Lan => *lan_permission,
        PathClass::Unknown => unreachable!(),
    };

    if matches!(input.path_class, PathClass::Tailnet) {
        match &input.whois {
            WhoIsOutcome::Ok {
                login, stable_id, ..
            } => {
                if !input.policy.allowed_users.iter().any(|user| user == login) {
                    return denied(ErrorCode::PolicyDenied, "tailnet user is not allowed");
                }
                if paired_node_stable_id.as_deref().is_some_and(|expected| {
                    expected != stable_id
                        && bound_node_stable_id.as_deref() != Some(stable_id.as_str())
                }) {
                    node_mismatch = true;
                    warnings.push(AuthWarning::NodeMismatch);
                }
                // A pairing can optionally pin the user as well. A mismatch is
                // treated like the node mismatch and requires confirmation.
                if paired_whois_user
                    .as_deref()
                    .is_some_and(|expected| expected != login)
                {
                    node_mismatch = true;
                    if !warnings.contains(&AuthWarning::NodeMismatch) {
                        warnings.push(AuthWarning::NodeMismatch);
                    }
                }
            }
            WhoIsOutcome::Unavailable => {
                warnings.push(AuthWarning::WhoIsUnavailable);
                effective_path = PathClass::Intranet;
                effective_permission = *intranet_permission;
            }
            WhoIsOutcome::NotApplicable => {}
        }
    }

    // INTRANET/LAN, including the tailscaled-unavailable fallback, checks the
    // path-specific permission before the host-wide switch.
    if matches!(effective_path, PathClass::Intranet | PathClass::Lan)
        && matches!(effective_permission, PathPermission::Deny)
    {
        return denied(ErrorCode::IntranetDenied, "device is denied on this path");
    }

    // 7. The host-wide remote-control switch applies to every authenticated
    // remote role, including view-only sessions.
    if !input.policy.allow_remote_control {
        return denied(
            ErrorCode::RemoteControlDisabled,
            "remote control is disabled",
        );
    }

    // 8. Viewer capacity follows the current host configuration.
    if input.intent != Intent::Files
        && input.session.has_controller
        && input.session.viewer_count >= input.policy.max_viewers
    {
        return denied(ErrorCode::SessionFull, "session is full");
    }

    // 9. Determine the role before calculating approval reasons.
    let role = if input.intent == Intent::Files {
        if matches!(effective_permission, PathPermission::ViewOnly)
            || matches!(permission, Permission::ViewOnly)
        {
            return denied(
                ErrorCode::PolicyDenied,
                "view-only device cannot transfer files",
            );
        }
        Role::Viewer
    } else if input.intent == Intent::View
        || matches!(effective_permission, PathPermission::ViewOnly)
        || matches!(permission, Permission::ViewOnly)
        || input.session.has_controller
    {
        Role::Viewer
    } else {
        Role::Controller
    };

    // 10. Any of these conditions makes the otherwise accepted connection
    // pending local confirmation for 30 seconds (the caller owns the timer).
    let mut approval = Vec::new();
    if input.policy.attended_mode {
        approval.push(ApprovalReason::AttendedMode);
    }
    if input.intent != Intent::Files
        && *permission == Permission::Control
        && input.local_activity_within_60s
    {
        approval.push(ApprovalReason::LocalActivity);
    }
    if matches!(effective_path, PathClass::Intranet | PathClass::Lan)
        && (input.policy.intranet_confirm_each || input.trust_stale_24h)
    {
        approval.push(ApprovalReason::IntranetConfirmation);
    }
    if node_mismatch {
        approval.push(ApprovalReason::NodeMismatch);
    }
    if *require_confirm {
        approval.push(ApprovalReason::DeviceRequiresConfirmation);
    }

    // 11. No approval reason means an ordinary OK/accepted connection.
    AuthDecision::Accept {
        role,
        needs_approval: (!approval.is_empty()).then_some(approval),
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewer_capacity_uses_policy() {
        let mut input = AuthInput {
            intent: Intent::Control,
            local_addr: "127.0.0.1:0".parse().unwrap(),
            peer_addr: "127.0.0.1:0".parse().unwrap(),
            path_class: PathClass::Lan,
            device: DeviceStatus::paired(Permission::Control),
            whois: WhoIsOutcome::NotApplicable,
            policy: AuthPolicy {
                max_viewers: 3,
                ..Default::default()
            },
            trust_stale_24h: false,
            session: SessionSnapshot {
                has_controller: true,
                viewer_count: 3,
            },
            local_activity_within_60s: false,
        };
        assert!(matches!(
            decide(&input),
            AuthDecision::Denied {
                code: ErrorCode::SessionFull,
                ..
            }
        ));
        input.session.viewer_count = 2;
        assert!(matches!(
            decide(&input),
            AuthDecision::Accept {
                role: Role::Viewer,
                ..
            }
        ));
    }
}
