//! Wire and IPC integration for the host-wide participant roster.
use super::{activity, idle, stream, ConnCx};
use crate::{
    audit::{AuditEvent, AuditPath},
    auth, ipc,
    platform::Platform,
    session::{
        activity::sessions_changed_event,
        journal::{OpenSession, SessionJournal},
        manager::{
            Admission, ConnInfo, ConnKey, EndReason, HostServices, ResumeOutcome, SessionManager,
        },
        roster::{
            ParticipantInfo, ParticipantView, Roster, RosterConfig, RosterError, RosterRole,
            SessionId, SessionPrefs, LINGER_US,
        },
        streamgate::{resume_reply, ResumeReply},
        takeover,
    },
};
use rsp_core::{errors::ErrorCode, features, pb::PathKind};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
};

struct TcpRegistration {
    registry: rsp_transport::tcp::TcpTokenRegistry,
    token: Vec<u8>,
}
impl Drop for TcpRegistration {
    fn drop(&mut self) {
        self.registry.unregister_token(&self.token);
    }
}

pub struct Host {
    pub roster: Mutex<Roster>,
    tcp_tokens: Mutex<HashMap<ConnKey, TcpRegistration>>,
    pending_end: Mutex<HashMap<String, PendingEnd>>,
    journal: Mutex<SessionJournal>,
}
impl Host {
    pub fn new(s: &HostServices) -> Self {
        Self {
            tcp_tokens: Mutex::new(HashMap::new()),
            pending_end: Mutex::new(HashMap::new()),
            journal: Mutex::new(match (&s.audit, &s.state_dir) {
                (Some(_), Some(dir)) => SessionJournal::open(dir, s.clock.now_micros()),
                _ => SessionJournal::disabled(),
            }),
            roster: Mutex::new(Roster::new(RosterConfig {
                max_viewers: s.config.get().access.max_viewers,
                linger_us: LINGER_US,
            })),
        }
    }
}
#[derive(Default)]
pub struct Conn {}
#[derive(Debug)]
pub enum Event {}
pub const FEATURES: u64 = features::RESUME | features::MULTI_VIEWER;

pub(crate) fn register_tcp<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    key: ConnKey,
    registry: &rsp_transport::tcp::TcpTokenRegistry,
    token: Vec<u8>,
    conn: &rsp_transport::Conn,
) -> Result<(), rsp_transport::ConnError> {
    registry.register_token(token.clone(), conn)?;
    mgr.features()
        .roster
        .tcp_tokens
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(
            key,
            TcpRegistration {
                registry: registry.clone(),
                token,
            },
        );
    Ok(())
}

pub(crate) fn unregister_tcp<P: Platform>(mgr: &Arc<SessionManager<P>>, key: ConnKey) {
    mgr.features()
        .roster
        .tcp_tokens
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&key);
}

pub fn with_roster<P: Platform, R>(
    mgr: &Arc<SessionManager<P>>,
    f: impl FnOnce(&mut Roster) -> R,
) -> R {
    f(&mut mgr
        .features()
        .roster
        .roster
        .lock()
        .unwrap_or_else(|p| p.into_inner()))
}

fn journal<P: Platform>(mgr: &Arc<SessionManager<P>>) -> MutexGuard<'_, SessionJournal> {
    mgr.features()
        .roster
        .journal
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn roster_role(role: auth::Role) -> RosterRole {
    match role {
        auth::Role::Controller => RosterRole::Controller,
        auth::Role::Viewer => RosterRole::Viewer,
    }
}

fn auth_role(role: RosterRole) -> auth::Role {
    match role {
        RosterRole::Controller => auth::Role::Controller,
        RosterRole::Viewer => auth::Role::Viewer,
    }
}

fn input_paused<P: Platform>(mgr: &Arc<SessionManager<P>>) -> bool {
    activity::paused_watch(mgr).is_some_and(|watch| *watch.borrow())
}

pub fn broadcast_session_state<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let paused = input_paused(mgr);
    let conns = mgr.conns();
    let (messages, summaries) = with_roster(mgr, |roster| {
        let messages: Vec<_> = conns
            .iter()
            .filter_map(|conn| {
                let session = roster.session_of(conn.key)?;
                Some((conn.key, roster.session_state_for(&session, paused)?))
            })
            .collect();
        (messages, roster.summaries())
    });
    for (conn, state) in messages {
        mgr.send(conn, state);
    }
    mgr.services()
        .events
        .emit(sessions_changed_event(&summaries, paused));
    flush_pending_ends(mgr);
}

struct PendingEnd {
    fields: AuditFields,
    role: &'static str,
    since_us: u64,
    left_at_us: u64,
}

/// The caller must hold the roster lock (inside a `with_roster` closure), so
/// a concurrent resume cannot consume this session between `leave` and the
/// pending-end record.
fn record_linger<P: Platform>(mgr: &Arc<SessionManager<P>>, session_id: &str, end: PendingEnd) {
    let left_at_us = end.left_at_us;
    mgr.features()
        .roster
        .pending_end
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(session_id.to_owned(), end);
    journal(mgr).left(session_id, left_at_us);
}

fn flush_pending_ends<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let ended: Vec<(String, PendingEnd)> = with_roster(mgr, |roster| {
        let mut pending = mgr
            .features()
            .roster
            .pending_end
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let removed: Vec<_> = pending
            .keys()
            .filter(|id| roster.role_of(&SessionId((*id).clone())).is_none())
            .cloned()
            .collect();
        removed
            .into_iter()
            .filter_map(|id| pending.remove(&id).map(|end| (id, end)))
            .collect()
    });
    for (id, end) in ended {
        push_audit(
            mgr.services(),
            &end.fields,
            "session_end",
            mgr.now_us(),
            Some(duration_s(end.since_us, end.left_at_us)),
            serde_json::json!({"role": end.role}),
        );
        journal(mgr).end(&id);
    }
}

fn as_candidate(view: ParticipantView) -> takeover::Candidate {
    takeover::Candidate {
        session_id: view.session_id.0,
        device_id: view.info.device_id,
        device_name: view.info.device_name,
        user: view.info.user,
        can_control: view.info.can_control,
        path: view.info.path,
        since_us: view.since_us,
    }
}

pub fn seat<P: Platform>(mgr: &Arc<SessionManager<P>>) -> takeover::Seat {
    with_roster(mgr, |roster| {
        if let Some(controller) = roster.controller() {
            takeover::Seat::Occupied {
                controller: as_candidate(controller),
            }
        } else if let Some(former) = roster.lingering_controller() {
            takeover::Seat::Lingering {
                former: as_candidate(former),
            }
        } else {
            takeover::Seat::Empty {
                former_user: roster.last_controller_user(),
            }
        }
    })
}

pub fn candidate<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    key: ConnKey,
) -> Option<takeover::Candidate> {
    with_roster(mgr, |roster| {
        roster
            .participants()
            .into_iter()
            .find(|view| view.conn == Some(key))
            .map(as_candidate)
    })
}

pub fn handoff<P: Platform>(mgr: &Arc<SessionManager<P>>, to_session: &str) -> bool {
    let transition = with_roster(mgr, |roster| {
        let handoff = roster
            .set_controller(&SessionId(to_session.to_owned()))
            .ok()?;
        Some((
            handoff.from.as_ref().and_then(|id| roster.conn_of(id)),
            roster.conn_of(&handoff.to),
        ))
    });
    let Some((from, to)) = transition else {
        return false;
    };
    if let Some(from) = from {
        mgr.set_role(from, auth::Role::Viewer);
    }
    if let Some(to) = to {
        mgr.set_role(to, auth::Role::Controller);
    }
    broadcast_session_state(mgr);
    true
}

pub fn on_host_start<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let leftovers = journal(mgr).leftovers();
    for end in leftovers {
        push_audit(
            mgr.services(),
            &AuditFields {
                user: end.session.user,
                device_id: end.session.device_id,
                path: end.session.path,
            },
            "session_end",
            mgr.now_us(),
            Some(end.duration_s),
            serde_json::json!({
                "role": end.session.role,
                "recovered": true,
                "ended_at": end.end_at_us,
            }),
        );
    }
    journal(mgr).clear_leftovers();
    let weak = Arc::downgrade(mgr);
    let mut config = mgr.services().config.subscribe();
    tokio::spawn(async move {
        loop {
            let Some(mgr) = weak.upgrade() else {
                break;
            };
            let now = mgr.now_us();
            let (expired, deadline) = with_roster(&mgr, |roster| {
                roster.set_max_viewers(config.borrow_and_update().access.max_viewers);
                (roster.tick(now), roster.next_deadline())
            });
            if !expired.is_empty() {
                for e in expired {
                    idle::on_linger_expired(&mgr, &e.session_id.0, e.was_controller);
                }
                broadcast_session_state(&mgr);
            }
            if journal(&mgr).touch_due(now) {
                let live: HashSet<String> = with_roster(&mgr, |roster| {
                    roster
                        .participants()
                        .into_iter()
                        .filter(|view| view.conn.is_some())
                        .map(|view| view.session_id.0)
                        .collect()
                });
                journal(&mgr).touch(now, &live);
            }
            let delay = super::timer_delay(deadline, now);
            // Only the weak reference survives the wait, so the timer cannot keep a host alive.
            drop(mgr);
            tokio::select! {
                () = tokio::time::sleep(delay) => {},
                changed = config.changed() => {
                    if changed.is_err() {
                        break;
                    }
                },
            }
        }
    });
}

pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    if cx.info.intent == rsp_core::pb::Intent::Files {
        return;
    }
    let paused = input_paused(cx.mgr);
    let (since_us, state) = with_roster(cx.mgr, |roster| {
        let since_us = roster
            .participants()
            .into_iter()
            .find(|view| view.conn == Some(cx.key))
            .map(|view| view.since_us);
        (
            since_us,
            roster.session_state_for(&SessionId(cx.info.session_id.clone()), paused),
        )
    });
    if let Some(since_us) = since_us {
        // Keep the connection registry and IPC on the same admission timestamp,
        // including the original timestamp when a session resumes.
        cx.info.since_us = since_us;
        cx.sync_info();
    }
    if let Some(state) = state {
        cx.send(state);
    }
    if cx.info.resumed {
        cx.mgr
            .features()
            .roster
            .pending_end
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&cx.info.session_id);
        journal(cx.mgr).resumed(&cx.info.session_id, cx.now_us());
    }
    broadcast_session_state(cx.mgr);
    if !cx.info.resumed {
        audit_session(cx, "session_start", None);
        journal(cx.mgr).start(open_session(cx.info), cx.now_us());
    }
}
pub fn on_role_changed<P: Platform>(cx: &mut ConnCx<'_, P>, old: auth::Role) {
    if old != cx.info.role {
        push_audit(
            cx.mgr.services(),
            &AuditFields::from_info(cx.info),
            "role_change",
            cx.now_us(),
            None,
            serde_json::json!({"from": role_name(old), "to": role_name(cx.info.role)}),
        );
        journal(cx.mgr).set_role(&cx.info.session_id, role_name(cx.info.role));
    }
    broadcast_session_state(cx.mgr);
}
pub fn on_stream_started<P: Platform>(cx: &mut ConnCx<'_, P>) {
    if let (Some(target), Some(start)) = (stream::current_target(cx), cx.st.stream.last_start) {
        with_roster(cx.mgr, |roster| {
            let _ = roster.set_prefs(
                &SessionId(cx.info.session_id.clone()),
                SessionPrefs {
                    display_id: target.wire_id,
                    quality: start.quality,
                },
            );
        });
    }
}
pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_event<P: Platform>(_cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {}
}
pub fn deadline<P: Platform>(_cx: &ConnCx<'_, P>) -> Option<u64> {
    None
}
pub fn on_tick<P: Platform>(_cx: &mut ConnCx<'_, P>, _now_us: u64) {}
pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, reason: EndReason) {
    if cx.info.intent == rsp_core::pb::Intent::Files {
        return;
    }
    let now = cx.now_us();
    let ended = with_roster(cx.mgr, |roster| {
        match roster.leave(cx.key, reason.lingers(), now) {
            Some(left) if left.lingering => {
                // Store while holding the roster lock, before a concurrent
                // resume can consume and clear this pending end.
                let id = left.session_id.0.clone();
                record_linger(
                    cx.mgr,
                    &id,
                    PendingEnd {
                        fields: AuditFields::from_info(cx.info),
                        role: role_name(cx.info.role),
                        since_us: cx.info.since_us,
                        left_at_us: now,
                    },
                );
                false
            }
            Some(_) => true,
            None => roster
                .role_of(&SessionId(cx.info.session_id.clone()))
                .is_none(),
        }
    });
    if ended {
        audit_session(cx, "session_end", Some(duration_s(cx.info.since_us, now)));
        journal(cx.mgr).end(&cx.info.session_id);
    }
    broadcast_session_state(cx.mgr);
}

/// A connection that never reaches `on_closed` can still have admitted a
/// roster entry. Record its leave before `SessionManager::unregister` drops
/// the connection metadata needed for the audit event.
pub(crate) fn on_dropped<P: Platform>(mgr: &Arc<SessionManager<P>>, key: ConnKey) {
    let info = mgr.info(key);
    let now = mgr.now_us();
    let changed = with_roster(mgr, |roster| {
        let since_us = roster
            .participants()
            .into_iter()
            .find(|view| view.conn == Some(key))
            .map(|view| view.since_us);
        let Some(left) = roster.leave(key, true, now) else {
            return false;
        };
        if let Some(info) = info.as_ref().filter(|info| {
            info.intent != rsp_core::pb::Intent::Files && info.session_id == left.session_id.0
        }) {
            record_linger(
                mgr,
                &left.session_id.0,
                PendingEnd {
                    fields: AuditFields::from_info(info),
                    role: role_name(info.role),
                    since_us: since_us.unwrap_or(info.since_us),
                    left_at_us: now,
                },
            );
        }
        true
    });
    if changed {
        broadcast_session_state(mgr);
    }
}

fn duration_s(since_us: u64, until_us: u64) -> i64 {
    i64::try_from(until_us.saturating_sub(since_us) / 1_000_000).unwrap_or(i64::MAX)
}

fn role_name(role: auth::Role) -> &'static str {
    match role {
        auth::Role::Controller => "controller",
        auth::Role::Viewer => "viewer",
    }
}

fn open_session(info: &ConnInfo) -> OpenSession {
    let fields = AuditFields::from_info(info);
    OpenSession {
        session_id: info.session_id.clone(),
        user: fields.user,
        device_id: fields.device_id,
        path: fields.path,
        role: role_name(info.role).to_owned(),
        since_us: info.since_us,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AuditFields {
    pub user: Option<String>,
    pub device_id: String,
    pub path: Option<AuditPath>,
}

impl AuditFields {
    pub(crate) fn from_info(info: &ConnInfo) -> Self {
        Self {
            user: (!info.user.is_empty()).then(|| info.user.clone()),
            device_id: info.device_id.clone(),
            path: match info.path {
                PathKind::TailnetDirect => Some(AuditPath::TailnetDirect),
                PathKind::TailnetDerp => Some(AuditPath::TailnetDerp),
                PathKind::Intranet => Some(AuditPath::Intranet),
                PathKind::Lan => Some(AuditPath::Lan),
                _ => None,
            },
        }
    }
}

/// Skip when auditing is disabled; persistence errors must not end a session.
pub(crate) fn push_audit(
    services: &HostServices,
    fields: &AuditFields,
    event: &str,
    ts_us: u64,
    duration_s: Option<i64>,
    detail: serde_json::Value,
) {
    let Some(audit) = &services.audit else {
        return;
    };
    let record = AuditEvent {
        ts: ts_us,
        user: fields.user.clone(),
        device_id: Some(fields.device_id.clone()),
        host_id: None,
        server: None,
        event: event.to_owned(),
        path: fields.path.clone(),
        duration_s,
        detail: Some(detail),
    };
    if let Err(error) = audit.push(record) {
        tracing::warn!(%error, event, "persist audit failed");
    }
}

fn audit_session<P: Platform>(cx: &ConnCx<'_, P>, event: &str, duration_s: Option<i64>) {
    push_audit(
        cx.mgr.services(),
        &AuditFields::from_info(cx.info),
        event,
        cx.now_us(),
        duration_s,
        serde_json::json!({"role": role_name(cx.info.role)}),
    );
}

pub fn auth_snapshot<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    device_id: &str,
) -> auth::SessionSnapshot {
    with_roster(mgr, |roster| roster.auth_snapshot(device_id))
}
pub fn resume<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    key: ConnKey,
    token: &[u8],
    device_id: &str,
) -> ResumeOutcome {
    let result = with_roster(mgr, |roster| {
        roster.resume(key, token, device_id, mgr.now_us())
    });
    // Authentication consumes the roster token before admission. Invalidate its
    // input registration immediately, even for TCP -> QUIC or another listener.
    if let Ok(resumed) = &result {
        if let Some(old) = resumed.replaced {
            unregister_tcp(mgr, old);
        }
    }
    let admission = result.as_ref().ok().map(|a| Admission {
        session_id: a.session_id.0.clone(),
        role: auth_role(a.role),
        token: a.token,
        replaced: a.replaced.into_iter().collect(),
        resumed: true,
    });
    match resume_reply(result) {
        ResumeReply::Resumed { .. } => admission.map_or(
            ResumeOutcome::Deny(ErrorCode::Other),
            ResumeOutcome::Resumed,
        ),
        ResumeReply::FreshSession { .. } => ResumeOutcome::Fresh,
        ResumeReply::Deny { auth } => ResumeOutcome::Deny(
            auth.error
                .and_then(|e| u16::try_from(e.code).ok())
                .and_then(ErrorCode::from_u16)
                .unwrap_or(ErrorCode::Other),
        ),
    }
}
pub fn admit<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    info: &ConnInfo,
    requested: auth::Role,
) -> Result<Admission, ErrorCode> {
    if info.intent == rsp_core::pb::Intent::Files {
        let session_id = rsp_core::file::new_transfer_id().map_err(|_| ErrorCode::Other)?;
        let mut token = [0; 16];
        getrandom::fill(&mut token).map_err(|_| ErrorCode::Other)?;
        return Ok(Admission {
            session_id,
            role: auth::Role::Viewer,
            token,
            replaced: mgr
                .conns()
                .into_iter()
                .filter(|conn| {
                    conn.key != info.key
                        && conn.device_id == info.device_id
                        && conn.intent == rsp_core::pb::Intent::Files
                })
                .map(|conn| conn.key)
                .collect(),
            resumed: false,
        });
    }
    let a = with_roster(mgr, |roster| {
        roster.admit(
            info.key,
            ParticipantInfo {
                device_id: info.device_id.clone(),
                device_name: info.device_name.clone(),
                user: info.user.clone(),
                path: info.path,
                can_control: info.can_control,
            },
            roster_role(requested),
            mgr.now_us(),
        )
    })
    .map_err(|error| match error {
        RosterError::SessionFull => ErrorCode::SessionFull,
        _ => ErrorCode::Other,
    })?;
    Ok(Admission {
        session_id: a.session_id.0,
        role: auth_role(a.role),
        token: a.token,
        replaced: a.replaced.into_iter().collect(),
        resumed: false,
    })
}
pub fn summaries<P: Platform>(mgr: &Arc<SessionManager<P>>) -> Vec<ipc::SessionSummary> {
    with_roster(mgr, |roster| roster.summaries())
}

/// Number of sessions that still block an update.
///
/// Includes resumable sessions and admissions awaiting connection registration.
/// Includes connections waiting for local approval.
/// Expired entries do not block while the roster timer catches up. This query
/// does not remove entries or otherwise change the roster.
pub fn active_sessions<P: Platform>(mgr: &Arc<SessionManager<P>>) -> usize {
    with_roster(mgr, |roster| {
        let now = mgr.now_us();
        let conns = mgr.conns();
        conns.len()
            + roster
                .participants()
                .into_iter()
                .filter(|participant| match participant.conn {
                    Some(key) => !conns.iter().any(|conn| conn.key == key),
                    None => participant
                        .linger_deadline_us
                        .is_some_and(|deadline| deadline > now),
                })
                .count()
            + super::approval::pending_connections(mgr)
    })
}

pub fn disconnect<P: Platform>(mgr: &Arc<SessionManager<P>>, session_id: &str) -> bool {
    let session = SessionId(session_id.to_owned());
    let found = with_roster(mgr, |roster| {
        roster.role_of(&session)?;
        let conn = roster.conn_of(&session);
        if conn.is_none() {
            roster.remove(&session);
        }
        Some(conn)
    });
    match found {
        Some(Some(conn)) => mgr.end_nowait(conn, EndReason::LocalUser),
        Some(None) => {
            broadcast_session_state(mgr);
            true
        }
        None => false,
    }
}
pub fn set_view_only<P: Platform>(mgr: &Arc<SessionManager<P>>, session_id: &str) -> bool {
    let session = SessionId(session_id.to_owned());
    let result = with_roster(mgr, |roster| {
        roster.set_view_only(&session)?;
        Ok::<_, RosterError>(roster.conn_of(&session))
    });
    match result {
        Ok(conn) => {
            if let Some(conn) = conn {
                mgr.set_role(conn, auth::Role::Viewer);
            }
            broadcast_session_state(mgr);
            true
        }
        Err(_) => false,
    }
}
pub fn on_device_revoked<P: Platform>(mgr: &Arc<SessionManager<P>>, fp: [u8; 32]) {
    let device_id = rsp_core::ids::id_from_fp(&fp);
    let conns = with_roster(mgr, |roster| {
        let mut conns = Vec::new();
        for session in roster.sessions_for_device(&device_id) {
            // Invalidate the token before queuing the close, including when a
            // background disconnect races with this revocation.
            if let Some(conn) = roster.remove(&session) {
                conns.push(conn);
            }
        }
        conns
    });
    for conn in conns {
        mgr.end_nowait(conn, EndReason::Revoked);
    }
    broadcast_session_state(mgr);
}
