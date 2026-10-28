//! 协议错误码。见 docs/dev/10-appendix.md 错误码。

/// RSP 错误码；协议只传数字，界面显示 `RS-E<码>`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum ErrorCode {
    /// 电脑缺少系统权限（屏幕录制、辅助功能，或 macOS 定期确认没点）。
    MissingPermissions = 101,
    /// 电脑离线，而且没有可用的内网或局域网路。
    Offline = 110,
    /// 节点在线，但 Host 没在运行（心跳超过 90 s，7400 端口拒绝连接）。
    HostNotRunning = 120,
    /// 被策略拦下：ACL 没放行（UDP、TCP 都超时），或用户不在 `allowed_users`。
    PolicyDenied = 130,
    /// 这台设备不允许经内网连接（该路径权限为 deny）。
    IntranetDenied = 131,
    /// 电脑关闭了「允许被远程控制」。
    RemoteControlDisabled = 132,
    /// 电脑前的人拒绝了，或 30 秒没响应（`detail.reason = "timeout"`）。
    ApprovalDenied = 140,
    /// 接管请求被拒绝。
    TakeoverDenied = 141,
    /// 会话已满（1 个控制者 + 3 个观看者）。
    SessionFull = 142,
    /// 证书指纹不符：这个地址上是另一台电脑，或者 Host 重装过。
    FingerprintMismatch = 150,
    /// 只有内网路，本机却没有公司 VPN 接口。
    VpnMissing = 160,
    /// 有到内网网段的路由，但 UDP 和 TCP 7400 都超时。
    PortBlocked = 161,
    /// Host 版本太旧（握手时检查协议版本）。
    HostTooOld = 170,
    /// App 版本太旧。
    AppTooOld = 171,
    /// 本机已被作废。
    DeviceRevoked = 180,
    /// 这台设备还没和这台电脑配对。
    NotPaired = 181,
    /// Mac 停在登录窗口，或切到了别的用户（FileVault 已关，tailnet 在线）。
    LoginWindow = 190,
    /// Mac 重启后停在 FileVault 解锁界面，tailnet 不在线。
    FileVaultLocked = 191,
    /// 其他错误（内部错误、未知错误）。
    Other = 199,
}

impl ErrorCode {
    pub const fn code(self) -> u16 {
        self as u16
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingPermissions => "RS-E101",
            Self::Offline => "RS-E110",
            Self::HostNotRunning => "RS-E120",
            Self::PolicyDenied => "RS-E130",
            Self::IntranetDenied => "RS-E131",
            Self::RemoteControlDisabled => "RS-E132",
            Self::ApprovalDenied => "RS-E140",
            Self::TakeoverDenied => "RS-E141",
            Self::SessionFull => "RS-E142",
            Self::FingerprintMismatch => "RS-E150",
            Self::VpnMissing => "RS-E160",
            Self::PortBlocked => "RS-E161",
            Self::HostTooOld => "RS-E170",
            Self::AppTooOld => "RS-E171",
            Self::DeviceRevoked => "RS-E180",
            Self::NotPaired => "RS-E181",
            Self::LoginWindow => "RS-E190",
            Self::FileVaultLocked => "RS-E191",
            Self::Other => "RS-E199",
        }
    }

    pub const fn from_u16(code: u16) -> Option<Self> {
        match code {
            101 => Some(Self::MissingPermissions),
            110 => Some(Self::Offline),
            120 => Some(Self::HostNotRunning),
            130 => Some(Self::PolicyDenied),
            131 => Some(Self::IntranetDenied),
            132 => Some(Self::RemoteControlDisabled),
            140 => Some(Self::ApprovalDenied),
            141 => Some(Self::TakeoverDenied),
            142 => Some(Self::SessionFull),
            150 => Some(Self::FingerprintMismatch),
            160 => Some(Self::VpnMissing),
            161 => Some(Self::PortBlocked),
            170 => Some(Self::HostTooOld),
            171 => Some(Self::AppTooOld),
            180 => Some(Self::DeviceRevoked),
            181 => Some(Self::NotPaired),
            190 => Some(Self::LoginWindow),
            191 => Some(Self::FileVaultLocked),
            199 => Some(Self::Other),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ErrorCode;

    #[test]
    fn every_registered_error_round_trips() {
        let errors = [
            (ErrorCode::MissingPermissions, 101, "RS-E101"),
            (ErrorCode::Offline, 110, "RS-E110"),
            (ErrorCode::HostNotRunning, 120, "RS-E120"),
            (ErrorCode::PolicyDenied, 130, "RS-E130"),
            (ErrorCode::IntranetDenied, 131, "RS-E131"),
            (ErrorCode::RemoteControlDisabled, 132, "RS-E132"),
            (ErrorCode::ApprovalDenied, 140, "RS-E140"),
            (ErrorCode::TakeoverDenied, 141, "RS-E141"),
            (ErrorCode::SessionFull, 142, "RS-E142"),
            (ErrorCode::FingerprintMismatch, 150, "RS-E150"),
            (ErrorCode::VpnMissing, 160, "RS-E160"),
            (ErrorCode::PortBlocked, 161, "RS-E161"),
            (ErrorCode::HostTooOld, 170, "RS-E170"),
            (ErrorCode::AppTooOld, 171, "RS-E171"),
            (ErrorCode::DeviceRevoked, 180, "RS-E180"),
            (ErrorCode::NotPaired, 181, "RS-E181"),
            (ErrorCode::LoginWindow, 190, "RS-E190"),
            (ErrorCode::FileVaultLocked, 191, "RS-E191"),
            (ErrorCode::Other, 199, "RS-E199"),
        ];
        for (error, code, text) in errors {
            assert_eq!(error.code(), code);
            assert_eq!(error.as_str(), text);
            assert_eq!(ErrorCode::from_u16(code), Some(error));
        }
        for code in 0..=u16::MAX {
            assert_eq!(
                ErrorCode::from_u16(code).is_some(),
                errors.iter().any(|(_, registered, _)| *registered == code)
            );
        }
    }
}
