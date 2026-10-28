//! Pure tailnet routing policy for the mobile app.

use std::time::Duration;

use super::dto_core::{DevicePlatform, EmbeddedNodeSummary, TailnetRoute, TailnetRouteStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailnetMode {
    System,
    Embedded,
    Auto,
}

impl TailnetMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "system" => Some(Self::System),
            "embedded" => Some(Self::Embedded),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Embedded => "embedded",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodePhase {
    Down,
    Starting,
    NeedsLogin,
    Running,
}

pub fn phase_of(state: &str) -> NodePhase {
    match state {
        "running" => NodePhase::Running,
        "needsLogin" | "needsMachineAuth" => NodePhase::NeedsLogin,
        "noState" | "starting" => NodePhase::Starting,
        _ => NodePhase::Down,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyInput {
    pub mode: TailnetMode,
    pub available: bool,
    pub control_url_set: bool,
    pub system_tailnet_up: bool,
    pub app_active: bool,
    pub woken: bool,
    pub login_pending: bool,
    pub live_connections: bool,
    pub node_started: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopWhen {
    Never,
    Now,
    AfterGrace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub route: TailnetRoute,
    pub want_node: bool,
    pub stop: StopWhen,
}

pub fn decide(input: &PolicyInput) -> Decision {
    let ready = input.available && input.control_url_set;
    let embedded_route = match input.mode {
        TailnetMode::System => false,
        TailnetMode::Embedded => ready,
        TailnetMode::Auto => ready && !input.system_tailnet_up,
    };
    let route = if embedded_route {
        TailnetRoute::Embedded
    } else {
        TailnetRoute::System
    };
    let demand = input.app_active || input.woken || input.live_connections;
    let keep_live = input.node_started && input.live_connections;
    let keep_login = input.node_started && input.login_pending && route == TailnetRoute::Embedded;
    let want_node = (route == TailnetRoute::Embedded && demand) || keep_live || keep_login;
    let stop = if want_node || !input.node_started {
        StopWhen::Never
    } else if route == TailnetRoute::Embedded && !input.app_active {
        StopWhen::AfterGrace
    } else {
        StopWhen::Now
    };

    Decision {
        route,
        want_node,
        stop,
    }
}

pub fn route_status(
    mode: TailnetMode,
    route: TailnetRoute,
    available: bool,
    control_url_set: bool,
    node: Option<EmbeddedNodeSummary>,
) -> TailnetRouteStatus {
    TailnetRouteStatus {
        mode: mode.as_str().to_owned(),
        active: route,
        available,
        control_url_set,
        embedded: node,
    }
}

pub fn node_hostname(platform: DevicePlatform, device_id: Option<&str>) -> String {
    let prefix = match platform {
        DevicePlatform::Ios => "rs-ios-",
        DevicePlatform::Ipados => "rs-ipad-",
        DevicePlatform::Android => "rs-android-",
        DevicePlatform::Macos => "rs-mac-",
        DevicePlatform::Windows => "rs-win-",
    };
    let suffix: String = device_id
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .take(8)
        .collect();
    let suffix = if suffix.len() < 4 { "app" } else { &suffix };
    format!("{prefix}{suffix}")
}

pub const BACKGROUND_GRACE: Duration = Duration::from_secs(30);
pub const LIVE_RECHECK: Duration = Duration::from_secs(5);
pub const WAKE_WINDOW: Duration = Duration::from_secs(60);
pub const LOGIN_HOLD: Duration = Duration::from_secs(600);
pub const START_RETRY: Duration = Duration::from_secs(30);
