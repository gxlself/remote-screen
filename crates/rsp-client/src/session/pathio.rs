//! Path recovery actions owned by the session runner.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use rsp_core::clock::{unix_micros, ClockSync, PingSchedule};
use rsp_core::hid::InputMsg;
use rsp_core::pb;
use rsp_transport::dial::{Candidate, DialOutcome};
use rsp_transport::{CloseCode, TransportKind};
use tokio::time::{timeout, Instant};

use super::redial;
use super::{
    Connection, EndReason, Runner, SessionError, SessionEvent, SessionStateKind, StateTransition,
};
use crate::recovery::driver::{PathAction, PathDriver};
use crate::recovery::failover::{FailoverAction, GiveUp, BACKUP_ATTEMPT_TIMEOUT};
use crate::recovery::liveness::PathSig;
use crate::recovery::upgrade::UpgradeAction;

impl Runner {
    pub(super) async fn poll_path(&mut self) -> Result<Option<EndReason>, SessionError> {
        if !matches!(
            self.state(),
            SessionStateKind::Streaming | SessionStateKind::Reconnecting
        ) {
            return Ok(None);
        }
        let Some(driver) = &mut self.driver else {
            return Ok(None);
        };
        let now = Instant::now();
        let actions = {
            let mut slot = super::lock(&self.shared.resume_token);
            driver.poll(&mut slot, now)
        };
        self.apply_path_actions(actions).await
    }

    pub(super) fn path_inbound(&mut self) {
        if let Some(driver) = &mut self.driver {
            driver.on_inbound(self.current_conn_id, Instant::now());
        }
    }

    pub(super) fn path_pong(&mut self, t1_us: u64) {
        if let Some(driver) = &mut self.driver {
            driver.on_pong(t1_us, Instant::now());
        }
    }

    pub(super) async fn apply_path_actions(
        &mut self,
        actions: Vec<PathAction>,
    ) -> Result<Option<EndReason>, SessionError> {
        for action in actions {
            tracing::debug!(?action, "session path action");
            match action {
                PathAction::SendPing => {
                    let t1_us = unix_micros();
                    if let Err(error) = self.send(pb::Ping { t1_us }).await {
                        tracing::debug!(%error, "path probe failed");
                        let actions = self.conn_closed_actions(self.current_conn_id, None);
                        return Box::pin(self.apply_path_actions(actions)).await;
                    } else if let Some(driver) = &mut self.driver {
                        driver.on_ping_sent(t1_us, Instant::now());
                        super::lock(&self.shared.stats).pings_sent += 1;
                    }
                }
                PathAction::Failover(action) => {
                    if let Some(end) = self.apply_failover_action(action).await? {
                        return Ok(Some(end));
                    }
                }
                PathAction::AcquireBackup { exclude } => self.start_backup_acquire(exclude).await,
                PathAction::CloseBackup(id) => self.close_backup(id),
                PathAction::Lost => return Ok(Some(EndReason::ConnectionLost)),
                PathAction::Upgrade(action) => {
                    if let Some(end) = self.apply_upgrade_action(action).await? {
                        return Ok(Some(end));
                    }
                }
            }
        }
        Ok(None)
    }

    async fn apply_upgrade_action(
        &mut self,
        action: UpgradeAction,
    ) -> Result<Option<EndReason>, SessionError> {
        match action {
            UpgradeAction::ProbeQuic => return self.start_upgrade_probe().await,
            UpgradeAction::DropProbe => {
                let replaced = self.cancel_upgrade().await;
                if replaced
                    && self.state() == SessionStateKind::Streaming
                    && !self.driver.as_ref().is_some_and(PathDriver::is_switching)
                {
                    // The Host replaced the TCP primary, but QUIC never committed.
                    tracing::info!(event = "path.lost", reason = "upgrade_uncommitted");
                    let actions = self.conn_closed_actions(self.current_conn_id, None);
                    return Box::pin(self.apply_path_actions(actions)).await;
                }
            }
            UpgradeAction::ResumeOnQuic => self.start_upgrade_resume(),
            UpgradeAction::Commit => return self.commit_upgrade(true).await,
            UpgradeAction::CommitNewSession => return self.commit_upgrade(false).await,
        }
        Ok(None)
    }

    async fn start_upgrade_probe(&mut self) -> Result<Option<EndReason>, SessionError> {
        self.cancel_upgrade().await;
        let Some(context) = self
            .dial_context
            .as_ref()
            .map(redial::DialContext::quic_only)
        else {
            let actions = if let Some(driver) = &mut self.driver {
                driver.on_probe_failed(Instant::now())
            } else {
                Vec::new()
            };
            return Box::pin(self.apply_path_actions(actions)).await;
        };
        tracing::debug!("starting QUIC upgrade probe");
        let candidates = context.candidates.clone();
        self.upgrade_probe = Some(redial::DialTask::spawn(context, candidates));
        Ok(None)
    }

    pub(super) async fn upgrade_probed(
        &mut self,
        result: redial::DialResult,
    ) -> Result<Option<EndReason>, SessionError> {
        let now = Instant::now();
        let outcome = match result {
            Ok(outcome) => outcome.take(),
            Err(_) => {
                let actions = if let Some(driver) = &mut self.driver {
                    driver.on_probe_failed(now)
                } else {
                    Vec::new()
                };
                return Box::pin(self.apply_path_actions(actions)).await;
            }
        };
        if let Some((spare, _)) = outcome.backup {
            spare.close(CloseCode::Normal, b"upgrade spare discarded");
        }
        let Some(driver) = &mut self.driver else {
            outcome
                .primary
                .close(CloseCode::Normal, b"upgrade driver gone");
            return Ok(None);
        };
        if outcome.transport != TransportKind::Quic {
            outcome
                .primary
                .close(CloseCode::Normal, b"upgrade requires QUIC");
            let actions = driver.on_probe_failed(now);
            return Box::pin(self.apply_path_actions(actions)).await;
        }
        let sig = PathSig::new(&outcome.primary_path.kind, TransportKind::Quic);
        let id = driver.next_conn_id();
        let actions = driver.on_probe_ok(id, sig.clone(), now);
        self.upgrade_conn = Some((id, outcome.primary, sig));
        Box::pin(self.apply_path_actions(actions)).await
    }

    /// Resume on the candidate while the TCP primary keeps streaming.
    fn start_upgrade_resume(&mut self) {
        let Some((_, conn, sig)) = &self.upgrade_conn else {
            return;
        };
        let token = super::lock(&self.shared.resume_token)
            .token_for(&self.host_fingerprint, Instant::now())
            .map(|token| token.to_vec())
            .unwrap_or_default();
        let hello = self
            .options
            .hello_with(token, path_from_sig(&sig.path), pb::Transport::Quic)
            .encode_to_vec();
        self.upgrade_resume = Some(redial::ResumeTask::spawn(conn.clone(), hello));
    }

    pub(super) async fn upgrade_resumed(
        &mut self,
        result: redial::ResumeResult,
    ) -> Result<Option<EndReason>, SessionError> {
        let resumed = match result {
            Ok(resumed) => resumed,
            Err(error) => {
                // The Upgrader's resume timeout drops the candidate.
                tracing::debug!(%error, "QUIC upgrade resume failed");
                if let Some((_, conn, _)) = &self.upgrade_conn {
                    conn.close(CloseCode::Normal, b"upgrade resume failed");
                }
                return Ok(None);
            }
        };
        let (Some((id, _, _)), Some(driver)) = (&self.upgrade_conn, &mut self.driver) else {
            return Ok(None);
        };
        let actions = {
            let mut slot = super::lock(&self.shared.resume_token);
            driver.on_upgrade_auth_result(*id, &resumed.auth, &mut slot, Instant::now())
        };
        self.upgrade_ready = Some(resumed);
        let result = Box::pin(self.apply_path_actions(actions)).await;
        self.upgrade_ready = None;
        result
    }

    /// Promotes the candidate QUIC connection to the primary path.
    async fn commit_upgrade(&mut self, resumed: bool) -> Result<Option<EndReason>, SessionError> {
        let (Some((id, conn, sig)), Some(ready)) =
            (self.upgrade_conn.take(), self.upgrade_ready.take())
        else {
            return Ok(Some(EndReason::ConnectionLost));
        };
        let old_conn = std::mem::replace(&mut self.conn, Connection::Current(conn));
        let old_control = self.control.replace(super::Control::Current(ready.control));
        if !resumed {
            // The Host keeps the old TCP session; end it before starting anew.
            if let Some(super::Control::Current(control)) = &old_control {
                let goodbye = pb::Envelope::from(pb::Goodbye {
                    reason: pb::goodbye::Reason::UserClosed as i32,
                    message: String::new(),
                })
                .encode_to_vec();
                let _ = timeout(Duration::from_millis(500), async {
                    control.send(&goodbye).await?;
                    control.flush().await
                })
                .await;
            }
        }
        old_conn.close(CloseCode::Normal, b"upgraded to QUIC");
        drop(old_control);

        let auth = ready.auth;
        self.current_conn_id = id;
        self.closing_conn = None;
        self.primary_replaced = false;
        self.options.path = path_from_sig(&sig.path);
        self.options.transport = transport_from_kind(sig.transport);
        self.options.token = (!auth.session_token.is_empty()).then(|| auth.session_token.clone());
        if !auth.session_token.is_empty() {
            self.conn.token(auth.session_token.clone())?;
        }
        self.input = None;
        self.approval = None;
        self.clock = ClockSync::new(30);
        self.reset_reassembly();
        self.restart_uni_reader();
        let peer_path = ready
            .hello
            .as_ref()
            .and_then(|hello| hello.peer.as_ref())
            .map(|peer| pb::PathKind::try_from(peer.path).unwrap_or(pb::PathKind::PathUnspecified))
            .unwrap_or(self.options.path);
        self.collector.set_path(peer_path, self.options.transport);
        if let Some(hello) = ready.hello {
            self.server_features = hello.features;
            self.shared
                .peer_features
                .store(hello.features, Ordering::Release);
            self.emit(SessionEvent::ServerHello(hello));
        }
        let role = pb::Role::try_from(auth.role).unwrap_or(pb::Role::Unspecified);
        *super::lock(&self.shared.role) = role;
        self.emit(SessionEvent::Role(role));
        self.shared.resumed.store(resumed, Ordering::Release);
        self.send_start_stream().await?;
        tracing::info!(event = "path.switch", reason = "upgrade", resumed);
        Ok(None)
    }

    /// Stops any upgrade attempt. Returns whether the Host had already
    /// replaced the primary for it.
    pub(super) async fn cancel_upgrade(&mut self) -> bool {
        if let Some(task) = self.upgrade_probe.take() {
            task.cancel().await;
        }
        self.upgrade_resume = None;
        self.upgrade_ready = None;
        if let Some((_, conn, _)) = self.upgrade_conn.take() {
            conn.close(CloseCode::Normal, b"upgrade probe dropped");
        }
        std::mem::take(&mut self.primary_replaced)
    }

    /// While the upgrade resumes on QUIC, the Host replacing the primary
    /// (`Goodbye(REPLACED)`, close code 3, control EOF) does not end the
    /// session; the candidate's `AuthResult` or the resume timeout decides.
    pub(super) fn primary_replaced_by_upgrade(&mut self) -> bool {
        if !self
            .driver
            .as_ref()
            .is_some_and(PathDriver::expects_replacement)
        {
            return false;
        }
        if !self.primary_replaced {
            tracing::debug!("primary replaced during QUIC upgrade");
            self.primary_replaced = true;
            // Stop reading the old primary; its later errors are expected.
            self.closing_conn = Some(self.current_conn_id);
        }
        true
    }

    async fn apply_failover_action(
        &mut self,
        action: FailoverAction,
    ) -> Result<Option<EndReason>, SessionError> {
        match action {
            FailoverAction::EnterReconnecting { .. } => {
                self.ping_due = None;
                self.feedback_due = None;
                self.cancel_backup_task().await;
                if self.state() == SessionStateKind::Streaming {
                    self.advance(StateTransition::Disconnected)?;
                }
            }
            FailoverAction::FreezeVideo => {}
            FailoverAction::ResetReassembler => self.reset_reassembly(),
            FailoverAction::CloseConn(id) => self.close_conn(id),
            FailoverAction::ResumeOn { conn, token } => {
                if let Some(end) = self.resume_on(conn, token).await? {
                    return Ok(Some(end));
                }
            }
            FailoverAction::Redial { attempt } => self.start_redial(attempt).await?,
            FailoverAction::SendReleaseAll => {
                let bytes = InputMsg::ReleaseAll
                    .to_frame()
                    .map_err(|error| SessionError::Protocol(error.to_string()))?;
                self.input = Some(self.conn.open_input(self.options.legacy_quic_input).await?);
                if let Some(input) = &mut self.input {
                    input.send(&bytes).await?;
                }
            }
            FailoverAction::RequestKeyframe => {
                if self.state() == SessionStateKind::Streaming {
                    self.keyframe(pb::keyframe_request::Reason::Resume, 0)
                        .await?;
                }
            }
            FailoverAction::StartNewSession => {}
            FailoverAction::HandToApproval { approval_timeout_s } => {
                if self.state() == SessionStateKind::Reconnecting {
                    self.advance(StateTransition::ApprovalPending)?;
                }
                self.approval =
                    Some(Instant::now() + Duration::from_secs(u64::from(approval_timeout_s) + 5));
            }
            FailoverAction::Recovered {
                elapsed,
                resumed,
                slow,
            } => {
                if self.state() == SessionStateKind::Reconnecting {
                    self.advance(StateTransition::Authenticated)?;
                }
                self.approval = None;
                self.shared.resumed.store(resumed, Ordering::Release);
                self.send_start_stream().await?;
                tracing::info!(
                    event = "path.switch",
                    dur_ms = elapsed.as_millis() as u64,
                    resumed,
                    slow,
                );
            }
            FailoverAction::GiveUp(reason) => return Ok(Some(self.recovery_end(reason))),
        }
        Ok(None)
    }

    /// Restarts the stream on a recovered or upgraded primary.
    async fn send_start_stream(&mut self) -> Result<(), SessionError> {
        if self.options.intent == pb::Intent::Probe || self.options.intent == pb::Intent::Files {
            return Ok(());
        }
        let display_id = self
            .last_stream_config
            .as_ref()
            .map_or(self.options.display, |config| config.display_id);
        self.send(pb::StartStream {
            display_id,
            codec: self.options.start.codec as i32,
            max_width: self.options.start.max_width,
            fps: u32::from(self.options.start.fps),
            bitrate_kbps: self.options.start.bitrate_kbps,
            quality: self.options.quality as i32,
            ..Default::default()
        })
        .await?;
        self.feedback_due = Some(Instant::now() + Duration::from_millis(200));
        self.ping_due = Some(Instant::now() + Duration::from_millis(60));
        self.ping_schedule = PingSchedule::fast_starting_at(self.now_us() + 60_000);
        Ok(())
    }

    async fn start_redial(&mut self, attempt: u32) -> Result<(), SessionError> {
        self.cancel_redial_task().await;
        let Some(context) = self.dial_context.clone() else {
            let actions = if let Some(driver) = &mut self.driver {
                let mut tickets = super::lock(&self.shared.resume_token);
                driver.on_redial_failed(&mut tickets, Instant::now())
            } else {
                Vec::new()
            };
            let _ = Box::pin(self.apply_path_actions(actions)).await?;
            return Ok(());
        };
        tracing::debug!(attempt, "starting session redial");
        let candidates = context.candidates.clone();
        self.redial_task = Some(redial::DialTask::spawn(context, candidates));
        Ok(())
    }

    async fn start_backup_acquire(&mut self, exclude: PathSig) {
        self.cancel_backup_task().await;
        let Some(context) = self.dial_context.clone() else {
            if let Some(driver) = &mut self.driver {
                driver.on_backup_failed(Instant::now());
            }
            return;
        };
        let candidates: Vec<Candidate> = context
            .candidates
            .iter()
            .filter(|candidate| candidate.kind.original() != exclude.path.original())
            .cloned()
            .collect();
        if candidates.is_empty() {
            if let Some(driver) = &mut self.driver {
                driver.on_backup_failed(Instant::now());
            }
            return;
        }
        self.backup_task = Some(redial::DialTask::spawn(context, candidates));
    }

    pub(super) async fn redial_finished(
        &mut self,
        result: redial::DialResult,
    ) -> Result<Option<EndReason>, SessionError> {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(_) => {
                let actions = if let Some(driver) = &mut self.driver {
                    let mut tickets = super::lock(&self.shared.resume_token);
                    driver.on_redial_failed(&mut tickets, Instant::now())
                } else {
                    Vec::new()
                };
                return Box::pin(self.apply_path_actions(actions)).await;
            }
        };
        self.install_redial_outcome(outcome.take()).await
    }

    async fn install_redial_outcome(
        &mut self,
        outcome: DialOutcome,
    ) -> Result<Option<EndReason>, SessionError> {
        let primary_sig = PathSig::new(&outcome.primary_path.kind, outcome.transport);
        // Keep the spare guarded while ResumeOn opens the primary control
        // stream. Cancellation or an early session ending must close it too.
        let mut spare_guard = super::SetupGuard {
            conn: None,
            backup: outcome.backup.as_ref().map(|(conn, _)| conn.clone()),
        };
        let Some(driver) = &mut self.driver else {
            outcome
                .primary
                .close(CloseCode::Normal, b"redial driver gone");
            return Ok(Some(EndReason::ConnectionLost));
        };
        let primary_id = driver.next_conn_id();
        let actions = {
            let tickets = super::lock(&self.shared.resume_token);
            driver.on_redial_connected(primary_id, primary_sig.clone(), &tickets, Instant::now())
        };
        self.pending_conn = Some((primary_id, outcome.primary, primary_sig));
        if !actions.iter().any(|action| {
            matches!(action,
                PathAction::Failover(FailoverAction::ResumeOn { conn, .. }) if *conn == primary_id)
        }) {
            self.close_conn(primary_id);
        }
        if let Some(end) = Box::pin(self.apply_path_actions(actions)).await? {
            return Ok(Some(end));
        }
        if let Some((backup, candidate)) = outcome.backup {
            let actions = self.hold_backup(backup, &candidate);
            spare_guard.backup = None;
            return Box::pin(self.apply_path_actions(actions)).await;
        }
        Ok(None)
    }

    pub(super) async fn backup_finished(&mut self, result: redial::DialResult) {
        match result {
            Ok(outcome) => {
                let outcome = outcome.take();
                if let Some((backup, _)) = outcome.backup {
                    backup.close(CloseCode::Normal, b"backup spare discarded");
                }
                let actions = self.hold_backup(outcome.primary, &outcome.primary_path);
                for action in actions {
                    if let PathAction::CloseBackup(id) = action {
                        self.close_backup(id);
                    }
                }
            }
            Err(_) => {
                if let Some(driver) = &mut self.driver {
                    driver.on_backup_failed(Instant::now());
                }
            }
        }
    }

    fn hold_backup(&mut self, conn: rsp_transport::Conn, candidate: &Candidate) -> Vec<PathAction> {
        let sig = PathSig::new(&candidate.kind, conn.kind());
        let Some(driver) = &mut self.driver else {
            conn.close(CloseCode::Normal, b"backup driver gone");
            return Vec::new();
        };
        if conn.kind() == TransportKind::Quic {
            let _ = conn.set_keepalive(Duration::from_secs(15));
        }
        let id = driver.next_conn_id();
        let actions = driver.on_backup_ready(id, sig.clone(), Instant::now());
        if driver.backup() == Some(id) {
            if let Some(old_backup) = self.backup.replace(conn) {
                old_backup.close(CloseCode::Normal, b"backup path replaced");
            }
            self.backup_conn_id = Some(id);
            self.backup_sig = Some(sig);
        } else {
            conn.close(CloseCode::Normal, b"backup path rejected");
        }
        actions
    }

    async fn cancel_redial_task(&mut self) {
        if let Some(task) = self.redial_task.take() {
            task.cancel().await;
        }
    }

    pub(super) async fn cancel_backup_task(&mut self) {
        if let Some(task) = self.backup_task.take() {
            task.cancel().await;
        }
    }

    pub(super) async fn cancel_path_tasks(&mut self) {
        self.cancel_redial_task().await;
        self.cancel_backup_task().await;
        self.cancel_upgrade().await;
        if let Some((_, conn, _)) = self.pending_conn.take() {
            conn.close(CloseCode::Normal, b"pending path dropped");
        }
    }

    fn recovery_end(&mut self, reason: GiveUp) -> EndReason {
        if matches!(reason, GiveUp::TicketExpired) {
            // A redial can finish at the expiry boundary and produce
            // GiveUp directly from Failover::on_connected, before poll() has
            // had a chance to consume the slot.
            super::lock(&self.shared.resume_token).clear();
        }
        match reason {
            GiveUp::Closed(code) => EndReason::Close(code),
            GiveUp::NoTicket => {
                self.emit(SessionEvent::Error {
                    code: crate::ErrorCode::Other,
                    message: "path recovery failed: no ticket".into(),
                });
                EndReason::Error(crate::ErrorCode::Other)
            }
            GiveUp::TicketExpired => {
                self.emit(SessionEvent::Error {
                    code: crate::ErrorCode::Other,
                    message: "path recovery failed: ticket expired".into(),
                });
                EndReason::Error(crate::ErrorCode::Other)
            }
            GiveUp::Denied(code) => {
                self.emit(SessionEvent::Error {
                    code: crate::ErrorCode::Other,
                    message: format!("path recovery failed: denied Host code {code}"),
                });
                EndReason::Error(crate::ErrorCode::Other)
            }
        }
    }

    async fn resume_on(
        &mut self,
        id: crate::recovery::failover::ConnId,
        token: [u8; crate::recovery::failover::TOKEN_LEN],
    ) -> Result<Option<EndReason>, SessionError> {
        let connection = if self.backup_conn_id == Some(id) {
            self.backup_conn_id = None;
            self.backup.take().zip(self.backup_sig.take())
        } else {
            match self.pending_conn.take() {
                Some((pending_id, conn, sig)) if pending_id == id => Some((conn, sig)),
                pending => {
                    self.pending_conn = pending;
                    None
                }
            }
        };
        let Some((backup, sig)) = connection else {
            return Ok(Some(EndReason::ConnectionLost));
        };
        let mut setup = super::SetupGuard {
            conn: Some(Connection::Current(backup.clone())),
            backup: None,
        };
        let open = timeout(BACKUP_ATTEMPT_TIMEOUT, backup.open_control());
        tokio::pin!(open);
        let control = loop {
            tokio::select! {
                biased;
                command = self.commands.recv() => match command {
                    Some(super::Command::Close) | None => {
                        return Ok(Some(EndReason::Goodbye(pb::goodbye::Reason::UserClosed)));
                    }
                    Some(super::Command::CloseWith(reason)) => {
                        return Ok(Some(EndReason::Goodbye(reason)));
                    }
                    // Commands issued during recovery are dropped, as in run().
                    Some(_) => {}
                },
                result = &mut open => match result {
                    Ok(Ok(control)) => break control,
                    _ => {
                        let actions = self.conn_closed_actions(id, None);
                        return Box::pin(self.apply_path_actions(actions)).await;
                    }
                }
            }
        };
        self.conn = Connection::Current(backup.clone());
        setup.conn = None;
        self.closing_conn = None;
        self.current_conn_id = id;
        self.control = Some(super::Control::Current(control));
        self.options.path = path_from_sig(&sig.path);
        self.options.transport = transport_from_kind(sig.transport);
        self.options.token = Some(token.to_vec());
        self.hello_seen = false;
        self.server_features = 0;
        self.input = None;
        self.clock = ClockSync::new(30);
        self.reset_reassembly();
        self.restart_uni_reader();
        if let Err(error) = self.send(self.options.hello()).await {
            tracing::debug!(%error, "resume ClientHello failed");
            let actions = self.conn_closed_actions(id, None);
            return Box::pin(self.apply_path_actions(actions)).await;
        }
        Ok(None)
    }

    pub(super) fn conn_closed_actions(
        &mut self,
        id: crate::recovery::failover::ConnId,
        code: Option<u64>,
    ) -> Vec<PathAction> {
        // A close or failed write must only enter the driver once. Disable
        // the old transport branches until a new ResumeOn installs a path.
        if id == self.current_conn_id {
            self.close_conn(id);
        }
        let now = Instant::now();
        let Some(driver) = &mut self.driver else {
            return vec![PathAction::Lost];
        };
        let mut slot = super::lock(&self.shared.resume_token);
        driver.on_conn_closed(id, code, &mut slot, now)
    }

    pub(super) async fn transport_failure(
        &mut self,
        id: crate::recovery::failover::ConnId,
    ) -> Result<Option<EndReason>, SessionError> {
        if self.driver.is_none() {
            return Ok(Some(EndReason::ConnectionLost));
        }
        if self.closing_conn == Some(id)
            || (id == self.current_conn_id && self.primary_replaced_by_upgrade())
        {
            return Ok(None);
        }
        let actions = self.conn_closed_actions(id, None);
        Box::pin(self.apply_path_actions(actions)).await
    }

    fn close_conn(&mut self, id: crate::recovery::failover::ConnId) {
        if id == self.current_conn_id {
            self.closing_conn = Some(id);
            self.conn.close(CloseCode::Normal, b"path lost");
            if let Some(reader) = self.uni_reader.take() {
                reader.abort();
            }
        } else if self.backup_conn_id == Some(id) {
            self.close_backup(id);
        } else if let Some((pending_id, conn, sig)) = self.pending_conn.take() {
            if pending_id == id {
                conn.close(CloseCode::Normal, b"pending path dropped");
            } else {
                self.pending_conn = Some((pending_id, conn, sig));
            }
        }
    }

    fn close_backup(&mut self, id: crate::recovery::failover::ConnId) {
        if self.backup_conn_id == Some(id) {
            if let Some(conn) = self.backup.take() {
                conn.close(CloseCode::Normal, b"backup dropped");
            }
            self.backup_conn_id = None;
            self.backup_sig = None;
        }
    }

    fn reset_reassembly(&mut self) {
        let fps = self
            .last_stream_config
            .as_ref()
            .map_or(u32::from(self.options.start.fps), |config| config.fps);
        self.reassembler = super::Reassembler::with_fps(fps);
        self.reception = super::feedback::Reception::default();
        self.last_good_frame = 0;
        self.audio_rx.reset();
        self.reassembly_poll = None;
        self.processed_datagrams = 0;
    }

    pub(super) fn restart_uni_reader(&mut self) {
        if let Some(reader) = self.uni_reader.take() {
            reader.abort();
        }
        let Connection::Current(conn) = &self.conn else {
            return;
        };
        let conn = conn.clone();
        let events = self.events.clone();
        let accept_files = self.options.features & rsp_core::features::FILE_TRANSFER != 0;
        self.uni_reader = Some(tokio::spawn(super::uni_reader(
            conn,
            events,
            Arc::clone(&self.shared),
            accept_files,
        )));
    }
}

fn path_from_sig(path: &rsp_transport::dial::PathKind) -> pb::PathKind {
    match path.original() {
        rsp_transport::dial::PathKind::Lan => pb::PathKind::Lan,
        rsp_transport::dial::PathKind::Intranet => pb::PathKind::Intranet,
        rsp_transport::dial::PathKind::Tailnet => pb::PathKind::TailnetDirect,
        _ => pb::PathKind::Manual,
    }
}

fn transport_from_kind(kind: TransportKind) -> pb::Transport {
    match kind {
        TransportKind::Quic => pb::Transport::Quic,
        TransportKind::Tcp => pb::Transport::Tcp,
    }
}
