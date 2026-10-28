//! RsCore 事件与控制器回调 → Tauri 事件、会话菜单、主窗口。
//!
//! 回调可能在 RsCore 的事件线程上（不得阻塞），也可能在主线程上（平台窗口事件）。
//! 菜单和窗口操作都经 `run_on_main_thread`：在主线程上直接执行，在别的线程上只投递。

use rs_desktop::{
    bridge::{CoreEvent, EventForward},
    controller::ControllerObserver,
    platform::SessionMenu,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::{lock, menu, AppState};

pub const RSCORE_EVENT: &str = "rscore://event";
pub const SESSION_EVENT: &str = "desktop://session";

/// `rscore://event` 的载荷；`seq` 是十进制 u64。
#[derive(Debug, Clone, Serialize)]
pub struct RsCoreEventPayload {
    pub seq: String,
    pub payload: String,
}

/// `desktop://session` 的载荷（`DesktopSessionEvent`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionEvent {
    Ended {
        session_id: String,
        host_id: String,
        by_user: bool,
    },
    Focus {
        session_id: String,
        focused: bool,
    },
}

/// 原始 JSON 原样转给前端；`typed` 由 `DesktopSessions` 自己订阅。
pub struct EventForwarder {
    pub app: AppHandle,
}

impl EventForward for EventForwarder {
    fn raw(&self, seq: u64, payload: &str) {
        let event = RsCoreEventPayload {
            seq: seq.to_string(),
            payload: payload.to_owned(),
        };
        if self.app.emit(RSCORE_EVENT, event).is_err() {
            tracing::warn!("rscore event not delivered");
        }
    }

    fn typed(&self, _event: &CoreEvent) {}
}

pub struct Observer {
    pub app: AppHandle,
}

impl Observer {
    fn emit(&self, event: SessionEvent) {
        if self.app.emit(SESSION_EVENT, event).is_err() {
            tracing::warn!("desktop session event not delivered");
        }
    }
}

impl ControllerObserver for Observer {
    fn menu_changed(&self, session_id: u64, menu: &SessionMenu) {
        let Some(state) = self.app.try_state::<AppState>() else {
            return;
        };
        {
            let mut focused = lock(&state.focused_session);
            match focused.as_mut() {
                Some((id, current)) if *id == session_id => *current = menu.clone(),
                _ => return,
            }
        }
        menu::refresh_session_later(&self.app);
    }

    fn focus_changed(&self, session_id: u64, focused: bool) {
        self.emit(SessionEvent::Focus {
            session_id: session_id.to_string(),
            focused,
        });
        let Some(state) = self.app.try_state::<AppState>() else {
            return;
        };
        let next = if focused {
            state
                .sessions
                .focused_menu()
                .filter(|(id, _)| *id == session_id)
        } else {
            None
        };
        {
            let mut current = lock(&state.focused_session);
            let is_current = current.as_ref().is_some_and(|(id, _)| *id == session_id);
            if !focused && !is_current {
                return;
            }
            *current = next;
        }
        menu::refresh_session_later(&self.app);
    }

    fn ended(&self, session_id: u64, host_id: &str, by_user: bool) {
        self.emit(SessionEvent::Ended {
            session_id: session_id.to_string(),
            host_id: host_id.to_owned(),
            by_user,
        });
        let Some(state) = self.app.try_state::<AppState>() else {
            return;
        };
        lock(&state.ended).insert(session_id);
        if by_user {
            // 用户关掉的窗口已移出窗口表，不一定还会收到失焦。
            let mut focused = lock(&state.focused_session);
            if focused.as_ref().is_some_and(|(id, _)| *id == session_id) {
                *focused = None;
            }
        }
        menu::refresh_session_later(&self.app);
        let app = self.app.clone();
        let posted = self.app.run_on_main_thread(move || {
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            if state.shutting_down() {
                return;
            }
            let Some(main) = app.get_webview_window(crate::MAIN_WINDOW) else {
                return;
            };
            if !main.is_visible().unwrap_or(true) && !state.has_live_sessions() {
                crate::show_main(&app);
            }
        });
        if posted.is_err() {
            tracing::warn!("main window check not scheduled");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_event_json_matches_frontend() {
        let ended = SessionEvent::Ended {
            session_id: "42".to_owned(),
            host_id: "h7v2m9q3k8x4tb1a".to_owned(),
            by_user: true,
        };
        assert_eq!(
            serde_json::to_value(ended).expect("serialize"),
            serde_json::json!({
                "type": "ended",
                "sessionId": "42",
                "hostId": "h7v2m9q3k8x4tb1a",
                "byUser": true,
            })
        );
        let focus = SessionEvent::Focus {
            session_id: "42".to_owned(),
            focused: false,
        };
        assert_eq!(
            serde_json::to_value(focus).expect("serialize"),
            serde_json::json!({"type": "focus", "sessionId": "42", "focused": false})
        );
        let raw = RsCoreEventPayload {
            seq: u64::MAX.to_string(),
            payload: "{}".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(raw).expect("serialize"),
            serde_json::json!({"seq": "18446744073709551615", "payload": "{}"})
        );
    }
}
