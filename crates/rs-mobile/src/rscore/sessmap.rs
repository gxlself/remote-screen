//! Pure session-event translation for the RsCore JSON boundary.

use std::collections::BTreeMap;

use rsp_client::{session::DialReport, SessionEvent, SessionStateKind};
use rsp_core::{clipboard::ClipboardKind, clock::unix_micros, errors::ErrorCode, pb};
use rsp_transport::dial::{AttemptLog, AttemptResult, Candidate, DialPlan};

use super::api_diag::failure_priority;
use super::dto_core::{
    codec_from_pb, end_reason_to_error, id_str, net_path_from_dial, net_path_from_pb,
    phase_from_state, role_from_pb, transport_from_pb, ClipboardDirection, CoreEvent, DialAttempt,
    DialState, NetPath, Participant, ReconnectReason, Role, SessionDisplay, SessionPhase,
    SessionSummary, Transport,
};

fn dial_attempt(
    candidate: &Candidate,
    state: DialState,
    ms: Option<u32>,
    reason: Option<String>,
) -> DialAttempt {
    DialAttempt {
        path: net_path_from_dial(&candidate.kind),
        addr: candidate.addr.ip().to_string(),
        state,
        ms,
        reason,
    }
}

fn attempt_ms(log: &AttemptLog) -> Option<u32> {
    let micros = log.finished_at_us?.saturating_sub(log.started_at_us?);
    Some(u32::try_from(micros / 1_000).unwrap_or(u32::MAX))
}

/// Candidates a dial plan will try, followed by those it skips.
pub fn dial_from_plan(plan: &DialPlan) -> Vec<DialAttempt> {
    plan.candidates
        .iter()
        .map(|candidate| dial_attempt(candidate, DialState::Trying, None, None))
        .chain(plan.skipped.iter().map(|(candidate, skipped)| {
            dial_attempt(
                candidate,
                DialState::Skipped,
                None,
                Some(format!("{skipped:?}")),
            )
        }))
        .collect()
}

/// One entry per candidate address, in first-attempt order. Only the primary
/// connection is `ok`: the App shows the last `trying`/`ok` entry as the path.
pub fn dial_from_report(report: &DialReport) -> Vec<DialAttempt> {
    let mut groups: Vec<(&Candidate, Vec<&AttemptLog>)> = Vec::new();
    for log in &report.attempts {
        match groups
            .iter_mut()
            .find(|(candidate, _)| candidate.addr == log.candidate.addr)
        {
            Some((_, logs)) => logs.push(log),
            None => groups.push((&log.candidate, vec![log])),
        }
    }
    groups
        .into_iter()
        .map(|(candidate, logs)| {
            if report
                .primary
                .as_ref()
                .is_some_and(|primary| primary.addr == candidate.addr)
            {
                let ms = logs
                    .iter()
                    .copied()
                    .find(|log| log.transport == report.transport)
                    .and_then(attempt_ms);
                return dial_attempt(candidate, DialState::Ok, ms, None);
            }
            if logs
                .iter()
                .any(|log| log.result == AttemptResult::Connected)
            {
                return dial_attempt(candidate, DialState::Skipped, None, Some("Backup".into()));
            }
            let failure = logs
                .iter()
                .copied()
                .filter(|log| {
                    matches!(
                        log.result,
                        AttemptResult::FingerprintMismatch { .. }
                            | AttemptResult::Refused
                            | AttemptResult::TimedOut
                            | AttemptResult::TlsFailed
                            | AttemptResult::Failed
                    )
                })
                .min_by_key(|log| failure_priority(&log.result));
            if let Some(log) = failure {
                // The mismatching fingerprint stays out of the event, as in probe results.
                let reason = match &log.result {
                    AttemptResult::FingerprintMismatch { .. } => "FingerprintMismatch".to_owned(),
                    result => format!("{result:?}"),
                };
                return dial_attempt(candidate, DialState::Failed, attempt_ms(log), Some(reason));
            }
            let reason = if logs
                .iter()
                .any(|log| log.result == AttemptResult::Cancelled)
            {
                "Cancelled".to_owned()
            } else {
                logs.iter()
                    .find_map(|log| match log.result {
                        AttemptResult::Skipped(skipped) => Some(format!("{skipped:?}")),
                        _ => None,
                    })
                    .unwrap_or_else(|| "NotStarted".to_owned())
            };
            dial_attempt(candidate, DialState::Skipped, None, Some(reason))
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct MapperSnapshot {
    pub phase: SessionPhase,
    pub role: Option<Role>,
    pub path: Option<NetPath>,
    pub transport: Option<Transport>,
    pub derp_region: Option<String>,
    pub host_name: Option<String>,
    pub displays: Vec<SessionDisplay>,
    pub current_display: Option<u32>,
    pub stream: Option<pb::StreamConfig>,
    pub controller: Option<Participant>,
    pub viewers: Vec<Participant>,
    pub input_paused_local: bool,
    pub rate_reason: Option<String>,
    pub ended: bool,
}

// SessionPhase has no Default implementation in the shared DTO contract.
impl Default for MapperSnapshot {
    fn default() -> Self {
        Self {
            phase: SessionPhase::Connecting,
            role: None,
            path: None,
            transport: None,
            derp_region: None,
            host_name: None,
            displays: Vec::new(),
            current_display: None,
            stream: None,
            controller: None,
            viewers: Vec::new(),
            input_paused_local: false,
            rate_reason: None,
            ended: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionMapper {
    session_id: String,
    host_id: String,
    snapshot: MapperSnapshot,
    last_error: Option<(Option<u32>, String)>,
    pending_error_detail: Option<(ErrorCode, BTreeMap<String, String>, Option<u32>)>,
    resuming: bool,
    auth_resumed: Option<bool>,
    dial: Vec<DialAttempt>,
}

impl SessionMapper {
    pub fn new(session_id: u64, host_id: &str) -> Self {
        Self {
            session_id: id_str(session_id),
            host_id: host_id.to_owned(),
            snapshot: MapperSnapshot::default(),
            last_error: None,
            pending_error_detail: None,
            resuming: false,
            auth_resumed: None,
            dial: Vec::new(),
        }
    }

    pub fn set_dial_result(&mut self, path: NetPath, transport: Transport) {
        if !self.snapshot.ended {
            self.snapshot.path = Some(path);
            self.snapshot.transport = Some(transport);
        }
    }

    /// Attach dial results to later pre-streaming state events.
    pub fn set_dial(&mut self, dial: Vec<DialAttempt>) {
        if !self.snapshot.ended
            && !matches!(
                self.snapshot.phase,
                SessionPhase::Streaming | SessionPhase::Ended
            )
        {
            self.dial = dial;
        }
    }

    pub fn snapshot(&self) -> &MapperSnapshot {
        &self.snapshot
    }

    /// Begin a token based recovery connection. The old path and transport
    /// remain visible while the new connection authenticates.
    pub fn begin_resume(&mut self) -> CoreEvent {
        self.resuming = true;
        self.auth_resumed = None;
        self.dial.clear();
        self.snapshot.phase = SessionPhase::Reconnecting;
        CoreEvent::SessionState {
            session_id: self.session_id.clone(),
            host_id: self.host_id.clone(),
            phase: SessionPhase::Reconnecting,
            path: self.snapshot.path,
            transport: self.snapshot.transport,
            derp_region: self.snapshot.derp_region.clone(),
            dial: None,
            approval_timeout_s: None,
            resumed: None,
            reconnect_reason: Some(ReconnectReason::Resume),
        }
    }

    pub fn set_auth_resumed(&mut self, resumed: bool) {
        self.auth_resumed = Some(resumed);
    }

    pub fn apply_to_summary(&self, s: &mut SessionSummary) {
        s.phase = self.snapshot.phase;
        s.role = self.snapshot.role;
        s.path = self.snapshot.path;
        s.transport = self.snapshot.transport;
        s.display_id = self.snapshot.current_display;
        if let Some(name) = &self.snapshot.host_name {
            if !name.is_empty() {
                s.host_name.clone_from(name);
            }
        }
    }

    /// Materialize the current roster, once the session's role is known.
    pub fn role_event(&self) -> Option<CoreEvent> {
        Some(CoreEvent::SessionRole {
            session_id: self.session_id.clone(),
            role: self.snapshot.role?,
            controller: self.snapshot.controller.clone(),
            viewers: self.snapshot.viewers.clone(),
            input_paused_local: self.snapshot.input_paused_local,
        })
    }

    /// Materialize the display list and remember its initial default selection.
    pub fn displays_event(&mut self) -> CoreEvent {
        self.snapshot.current_display = self.snapshot.current_display.or_else(|| {
            self.snapshot
                .displays
                .iter()
                .find(|display| display.primary)
                .or_else(|| self.snapshot.displays.first())
                .map(|display| display.id)
        });
        CoreEvent::SessionDisplays {
            session_id: self.session_id.clone(),
            displays: self.snapshot.displays.clone(),
            current_id: self.snapshot.current_display.unwrap_or(0),
        }
    }

    pub fn map(&mut self, ev: &SessionEvent) -> Vec<CoreEvent> {
        if self.snapshot.ended {
            return Vec::new();
        }

        // Context belongs only to the immediately following matching Error.
        let pending_error_detail = self.pending_error_detail.take();
        let mut events = Vec::new();
        match ev {
            SessionEvent::StateChanged(state) => {
                let previous = self.snapshot.phase;
                let phase = phase_from_state(*state);
                if self.resuming
                    && matches!(
                        phase,
                        SessionPhase::Connecting | SessionPhase::Authenticating
                    )
                {
                    return events;
                }
                if self.resuming
                    && matches!(
                        phase,
                        SessionPhase::Streaming | SessionPhase::PendingApproval
                    )
                {
                    self.resuming = false;
                }
                if previous != phase {
                    self.snapshot.phase = phase;
                    let dial = if matches!(
                        phase,
                        SessionPhase::Streaming | SessionPhase::Ended | SessionPhase::Reconnecting
                    ) {
                        self.dial.clear();
                        None
                    } else {
                        (!self.dial.is_empty()).then(|| self.dial.clone())
                    };
                    events.push(CoreEvent::SessionState {
                        session_id: self.session_id.clone(),
                        host_id: self.host_id.clone(),
                        phase,
                        path: self.snapshot.path,
                        transport: self.snapshot.transport,
                        derp_region: self.snapshot.derp_region.clone(),
                        dial,
                        // SessionEvent does not expose AuthResult.approval_timeout_s.
                        approval_timeout_s: (phase == SessionPhase::PendingApproval).then_some(30),
                        resumed: if phase == SessionPhase::Streaming
                            && previous == SessionPhase::Reconnecting
                        {
                            self.auth_resumed.take().or(Some(true))
                        } else {
                            None
                        },
                        reconnect_reason: (phase == SessionPhase::Reconnecting)
                            .then_some(ReconnectReason::PathLost),
                    });
                }
            }
            SessionEvent::ServerHello(hello) => {
                self.snapshot.host_name = Some(hello.host_name.clone());
                if let Some(peer) = &hello.peer {
                    if let Some(path) = net_path_from_pb(peer.path()) {
                        self.snapshot.path = Some(path);
                    }
                    if !peer.derp_region.is_empty() {
                        self.snapshot.derp_region = Some(peer.derp_region.clone());
                    }
                }
                if !hello.displays.is_empty() && self.snapshot.displays.is_empty() {
                    self.snapshot.displays =
                        hello.displays.iter().map(SessionDisplay::from).collect();
                    events.push(self.displays_event());
                }
            }
            SessionEvent::SessionState(state) => {
                self.snapshot.role = role_from_pb(state.your_role());
                self.snapshot.controller = state.controller.as_ref().map(Participant::from);
                self.snapshot.viewers = state.viewers.iter().map(Participant::from).collect();
                self.snapshot.input_paused_local = state.input_paused_local;
                events.extend(self.role_event());
            }
            SessionEvent::Role(role) => {
                let role = role_from_pb(*role);
                if self.snapshot.role != role {
                    self.snapshot.role = role;
                    events.extend(self.role_event());
                }
            }
            SessionEvent::LocalUse(paused) => {
                if self.snapshot.input_paused_local != *paused {
                    self.snapshot.input_paused_local = *paused;
                    events.extend(self.role_event());
                }
            }
            SessionEvent::Displays(displays) => {
                self.snapshot.displays = displays.iter().map(SessionDisplay::from).collect();
                events.push(self.displays_event());
            }
            SessionEvent::VirtualDisplay(result) => {
                events.extend(super::vdisplay::result_event(&self.session_id, result));
            }
            SessionEvent::StreamConfig(config) => {
                self.snapshot.stream = Some(*config);
                if self.snapshot.current_display != Some(config.display_id) {
                    self.snapshot.current_display = Some(config.display_id);
                    if !self.snapshot.displays.is_empty() {
                        events.push(self.displays_event());
                    }
                }
            }
            SessionEvent::ControlRequested(request) => {
                events.push(CoreEvent::SessionControlRequest {
                    session_id: self.session_id.clone(),
                    requester: Participant::from(
                        request
                            .requester
                            .as_ref()
                            .unwrap_or(&pb::Participant::default()),
                    ),
                    timeout_s: request.timeout_s,
                });
            }
            SessionEvent::IdleWarning(warning) => {
                events.push(CoreEvent::SessionIdleWarning {
                    session_id: self.session_id.clone(),
                    seconds_left: warning.seconds_left,
                    lock_on_end: warning.lock_on_end,
                });
            }
            SessionEvent::RateUpdate(rate) => {
                if !rate.reason.is_empty() {
                    self.snapshot.rate_reason = Some(rate.reason.clone());
                }
            }
            SessionEvent::HostStats(stats) => {
                if let Some(path) = net_path_from_pb(stats.path()) {
                    self.snapshot.path = Some(path);
                }
                if !stats.derp_region.is_empty() {
                    self.snapshot.derp_region = Some(stats.derp_region.clone());
                }
            }
            SessionEvent::Clipboard(item) => {
                if item.kind == ClipboardKind::Text {
                    if let Ok(text) = std::str::from_utf8(&item.data) {
                        events.push(CoreEvent::SessionClipboard {
                            session_id: self.session_id.clone(),
                            direction: ClipboardDirection::FromHost,
                            text: text.to_owned(),
                            at: unix_micros(),
                        });
                    }
                }
            }
            SessionEvent::Stats(stats) => {
                let Some(hud) = &stats.hud else {
                    return events;
                };
                if let Some(path) = net_path_from_pb(hud.path) {
                    self.snapshot.path = Some(path);
                }
                if let Some(transport) = transport_from_pb(hud.transport) {
                    self.snapshot.transport = Some(transport);
                }
                if let (Some(stream), Some(path), Some(transport)) = (
                    &self.snapshot.stream,
                    self.snapshot.path,
                    self.snapshot.transport,
                ) {
                    if let Some(codec) = codec_from_pb(stream.codec()) {
                        let rtt_us = if hud.rtt_us != 0 {
                            u64::from(hud.rtt_us)
                        } else {
                            stats.rtt_us.unwrap_or(0)
                        };
                        events.push(CoreEvent::SessionStats {
                            session_id: self.session_id.clone(),
                            // Contract gap: dto_core currently requires u32 milliseconds.
                            // Fractional RTT needs CoreEvent::SessionStats.rtt_ms: f64.
                            rtt_ms: (rtt_us / 1_000).min(u64::from(u32::MAX)) as u32,
                            fps: hud.fps,
                            bitrate_kbps: hud.recv_kbps,
                            decode_ms: ((f64::from(hud.decode_p50_us) / 1_000.0) * 10.0).round()
                                / 10.0,
                            loss_pct: (f64::from(hud.loss_pct) * 10.0).round() / 10.0,
                            width: stream.width,
                            height: stream.height,
                            codec,
                            path,
                            transport,
                            derp_region: self.snapshot.derp_region.clone(),
                            rate_reason: self.snapshot.rate_reason.clone(),
                        });
                    }
                }
            }
            SessionEvent::ErrorDetail {
                code,
                detail,
                retry_after_s,
            } => {
                self.pending_error_detail = Some((*code, detail.clone(), *retry_after_s));
            }
            SessionEvent::Error { code, message } => {
                let (detail, retry_after_s) = pending_error_detail
                    .filter(|(detail_code, _, _)| detail_code == code)
                    .map(|(_, detail, retry)| ((!detail.is_empty()).then_some(detail), retry))
                    .unwrap_or_default();
                let code = Some(u32::from(code.code()));
                if self
                    .last_error
                    .as_ref()
                    .is_none_or(|(last_code, last_message)| {
                        *last_code != code || last_message != message
                    })
                {
                    events.push(CoreEvent::SessionError {
                        session_id: self.session_id.clone(),
                        host_id: self.host_id.clone(),
                        code,
                        reason: None,
                        message: Some(message.clone()),
                        detail,
                        retry_after_s,
                    });
                    self.last_error = Some((code, message.clone()));
                }
            }
            SessionEvent::Ended { code, reason, kind } => {
                let (fallback_code, goodbye_reason) = end_reason_to_error(kind);
                let code = code.map(|code| u32::from(code.code())).or(fallback_code);
                if goodbye_reason.is_some()
                    || self
                        .last_error
                        .as_ref()
                        .is_none_or(|(last_code, _)| *last_code != code)
                {
                    events.push(CoreEvent::SessionError {
                        session_id: self.session_id.clone(),
                        host_id: self.host_id.clone(),
                        code,
                        reason: goodbye_reason,
                        message: Some(reason.clone()),
                        detail: None,
                        retry_after_s: None,
                    });
                }
                events.extend(self.map(&SessionEvent::StateChanged(SessionStateKind::Ended)));
                // StateChanged(Ended) can precede the typed terminal event in rsp-client.
                // Only the terminal event freezes mapping, so its reason is preserved.
                self.snapshot.ended = true;
            }
            SessionEvent::PairResult(_)
            | SessionEvent::Frame(_)
            | SessionEvent::Cursor(_)
            | SessionEvent::CursorMessage(_)
            | SessionEvent::ClockSync { .. } => {}
            // FILES sessions are mapped by the transfer layer, not the session map.
            SessionEvent::FileStream(_)
            | SessionEvent::FileListing(_)
            | SessionEvent::FileStatus(_)
            | SessionEvent::FileCancel(_) => {}
        }
        events
    }
}
