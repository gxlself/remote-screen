//! Host-side handling for App recording notices (M3 V5, privacy plan A).
//!
//! A reconnect can split one local recording into multiple audit events: the
//! old connection finishes its interval when it closes, and the resumed
//! connection starts a new interval when the App repeats its active notice.

use super::ConnCx;
use crate::{
    auth,
    platform::Platform,
    session::{
        handlers::roster,
        manager::{EndReason, HostServices, SessionManager},
        roster::SessionId,
    },
};
use rsp_core::{features, pb};
use serde_json::json;
use std::sync::Arc;

pub struct Host;

impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self
    }
}

#[derive(Default)]
pub struct Conn {
    pub recording_since_us: Option<u64>,
}

#[derive(Debug)]
pub enum Event {}

pub const FEATURES: u64 = features::RECORDING_NOTICE;

pub fn on_host_start<P: Platform>(_mgr: &Arc<SessionManager<P>>) {}

pub fn on_admitted<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}

pub fn on_stream_started<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_event<P: Platform>(_cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {}
}

pub fn deadline<P: Platform>(_cx: &ConnCx<'_, P>) -> Option<u64> {
    None
}

pub fn on_tick<P: Platform>(_cx: &mut ConnCx<'_, P>, _now_us: u64) {}

pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    if cx.st.recording.recording_since_us.is_some() {
        finish(cx);
    }
}

pub fn on_notice<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::RecordingNotice) {
    match (m.active, cx.st.recording.recording_since_us) {
        (true, None) => {
            let since = cx.now_us();
            cx.st.recording.recording_since_us = Some(since);
            let changed = roster::with_roster(cx.mgr, |roster| {
                roster
                    .session_of(cx.key)
                    .map(|session| roster.set_recording(&session, true))
                    .unwrap_or(false)
            });
            if changed {
                roster::broadcast_session_state(cx.mgr);
            }
            tracing::info!(role = ?cx.info.role, "recording notice started");
        }
        (true, Some(_)) => {}
        (false, Some(_)) => finish(cx),
        (false, None) => {}
    }
}

fn finish<P: Platform>(cx: &mut ConnCx<'_, P>) {
    let Some(since) = cx.st.recording.recording_since_us.take() else {
        return;
    };
    let now = cx.now_us();
    let duration_s = i64::try_from(now.saturating_sub(since) / 1_000_000).unwrap_or(i64::MAX);
    let role = role_name(cx.info.role);
    roster::push_audit(
        cx.mgr.services(),
        &roster::AuditFields::from_info(cx.info),
        "recording",
        now,
        Some(duration_s),
        json!({"role": role}),
    );

    // Roster teardown runs before this hook. Clear a lingering entry by its
    // session id, while avoiding a late close from clearing a replacement's
    // active recording marker.
    let changed = roster::with_roster(cx.mgr, |roster| {
        let session = SessionId(cx.info.session_id.clone());
        match roster.conn_of(&session) {
            Some(key) if key != cx.key => false,
            _ => roster.set_recording(&session, false),
        }
    });
    if changed {
        roster::broadcast_session_state(cx.mgr);
    }
}

fn role_name(role: auth::Role) -> &'static str {
    match role {
        auth::Role::Controller => "controller",
        auth::Role::Viewer => "viewer",
    }
}
