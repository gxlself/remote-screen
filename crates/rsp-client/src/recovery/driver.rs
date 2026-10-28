//! Composition of the liveness, backup, failover, and upgrade state machines.
//!
//! `PathDriver` is deliberately a logic-only adapter. The session layer owns
//! connections and performs the actions returned here; every time value is
//! supplied by the caller.

use rsp_core::pb;
use rsp_transport::TransportKind;
use tokio::time::Instant;

use super::failover::{
    ConnId, Failover, FailoverAction, FailoverPhase, FrameVerdict, InputVerdict, ReconnectReason,
    TicketSlot,
};
use super::liveness::{BackupAction, BackupKeeper, Liveness, MonitorConfig, PathMonitor, PathSig};
use super::upgrade::{UpgradeAction, Upgrader};

/// Driver-wide options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DriverConfig {
    pub monitor: MonitorConfig,
    pub redial: bool,
}

/// An action for the session layer to perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathAction {
    SendPing,
    Failover(FailoverAction),
    AcquireBackup { exclude: PathSig },
    CloseBackup(ConnId),
    Lost,
    Upgrade(UpgradeAction),
}

#[derive(Debug, Clone)]
struct HeldBackup {
    conn: ConnId,
    sig: PathSig,
}

/// Pure composition of the path recovery state machines.
#[derive(Debug)]
pub struct PathDriver {
    cfg: DriverConfig,
    primary_conn: ConnId,
    primary_sig: PathSig,
    backup: Option<HeldBackup>,
    pending: Option<(ConnId, PathSig)>,
    next_conn: u64,
    keeper: BackupKeeper,
    monitor: Option<PathMonitor>,
    failover: Failover,
    lost: bool,
    host_fp: [u8; 32],
    upgrader: Upgrader,
    candidate: Option<(ConnId, PathSig)>,
    upgrade_blocked: bool,
    suspended: bool,
    /// Latest caller-supplied time, for calls that do not take one.
    last_now: Instant,
}

impl PathDriver {
    /// Creates a driver with connection IDs one and two reserved for the
    /// supplied primary and backup. Newly dialled connections start at three.
    pub fn new(
        cfg: DriverConfig,
        host_fp: [u8; 32],
        primary: PathSig,
        backup: Option<PathSig>,
        now: Instant,
    ) -> Self {
        let backup = backup.map(|sig| HeldBackup {
            conn: ConnId(2),
            sig,
        });
        let keeper = BackupKeeper::new(
            primary.clone(),
            backup.as_ref().map(|held| held.sig.clone()),
            now,
        );
        // Upgrades wait for streaming; on_streaming lifts this block.
        let mut upgrader = Upgrader::new();
        upgrader.on_primary(primary.transport, now);
        upgrader.set_blocked(true, now);
        Self {
            cfg,
            primary_conn: ConnId(1),
            primary_sig: primary,
            backup,
            pending: None,
            next_conn: 3,
            keeper,
            monitor: None,
            failover: Failover::new(ConnId(1), host_fp),
            lost: false,
            host_fp,
            upgrader,
            candidate: None,
            upgrade_blocked: true,
            suspended: false,
            last_now: now,
        }
    }

    pub fn primary(&self) -> ConnId {
        self.primary_conn
    }

    pub fn primary_sig(&self) -> &PathSig {
        &self.primary_sig
    }

    pub fn backup(&self) -> Option<ConnId> {
        self.backup.as_ref().map(|held| held.conn)
    }

    pub fn next_conn_id(&mut self) -> ConnId {
        let id = ConnId(self.next_conn);
        self.next_conn = self.next_conn.saturating_add(1);
        id
    }

    /// Installs the monitor once, on the first transition into streaming.
    pub fn on_streaming(&mut self, now: Instant) {
        self.touch(now);
        if self.monitor.is_none() {
            self.monitor = Some(PathMonitor::new(self.cfg.monitor, now));
            // Unblocking never produces an action.
            let _ = self.sync_upgrade(now);
        }
    }

    pub fn on_inbound(&mut self, conn: ConnId, now: Instant) {
        if self.lost || conn != self.primary_conn {
            return;
        }
        if let Some(monitor) = &mut self.monitor {
            monitor.on_inbound(now);
        }
    }

    pub fn on_ping_sent(&mut self, t1_us: u64, now: Instant) {
        if self.lost {
            return;
        }
        if let Some(monitor) = &mut self.monitor {
            monitor.on_ping_sent(t1_us, now);
        }
    }

    pub fn on_pong(&mut self, t1_us: u64, now: Instant) {
        if self.lost {
            return;
        }
        if let Some(monitor) = &mut self.monitor {
            monitor.on_pong(t1_us, now);
        }
    }

    /// Polls monitor, failover, and (only while streaming) backup acquisition
    /// and the TCP-to-QUIC upgrade.
    ///
    /// The monitor is not polled while the upgrade resumes the session on
    /// QUIC: the Host may already have replaced the old primary, and the
    /// upgrade's own timeout decides that outcome.
    pub fn poll(&mut self, tickets: &mut TicketSlot, now: Instant) -> Vec<PathAction> {
        if self.lost {
            return Vec::new();
        }
        self.touch(now);

        let mut actions = Vec::new();

        if matches!(
            self.failover.phase(),
            FailoverPhase::Streaming | FailoverPhase::AwaitingKeyframe { .. }
        ) && !self.upgrader.expects_replacement()
        {
            if let Some(monitor) = &mut self.monitor {
                let result = monitor.poll(now);
                if result.send_probe {
                    actions.push(PathAction::SendPing);
                }
                if let Liveness::Dead { silent_for } = result.liveness {
                    tracing::info!(
                        event = "path.dead",
                        silent_ms = silent_for.as_millis() as u64,
                        has_backup = self.backup.is_some(),
                    );
                    let backup = self.take_backup_for_failover(now);
                    let backup_id = backup.as_ref().map(|(conn, _)| *conn);
                    let raw = self.failover.start(
                        ReconnectReason::PathLost,
                        self.primary_conn,
                        backup_id,
                        tickets,
                        now,
                    );
                    actions.extend(self.convert_failover(raw, now));
                }
            }
        }

        if self.lost {
            return actions;
        }

        let failover_actions = self.failover.poll(tickets, now);
        actions.extend(self.convert_failover(failover_actions, now));

        if self.lost {
            return actions;
        }

        if matches!(self.failover.phase(), FailoverPhase::Streaming) {
            if let Some(BackupAction::Acquire { exclude }) = self.keeper.poll(now) {
                actions.push(PathAction::AcquireBackup { exclude });
            }
        }

        actions.extend(self.sync_upgrade(now));
        let raw = self.upgrader.poll(now);
        actions.extend(self.convert_upgrade(raw));
        actions
    }

    pub fn on_conn_closed(
        &mut self,
        conn: ConnId,
        close_code: Option<u64>,
        tickets: &mut TicketSlot,
        now: Instant,
    ) -> Vec<PathAction> {
        if self.lost {
            return Vec::new();
        }
        self.touch(now);

        if self.backup.as_ref().is_some_and(|held| held.conn == conn) {
            self.backup = None;
            self.keeper.on_backup_lost(now);
            return Vec::new();
        }

        let backup_id = if matches!(
            self.failover.phase(),
            FailoverPhase::Streaming | FailoverPhase::AwaitingKeyframe { .. }
        ) && close_code.is_none()
            && conn == self.primary_conn
        {
            self.take_backup_for_failover(now)
                .as_ref()
                .map(|(id, _)| *id)
        } else {
            None
        };

        if self.pending.as_ref().is_some_and(|(id, _)| *id == conn) {
            self.pending = None;
        }
        let failover_actions = self
            .failover
            .on_conn_closed(conn, close_code, backup_id, tickets, now);
        self.convert_failover(failover_actions, now)
    }

    pub fn on_auth_result(
        &mut self,
        conn: ConnId,
        result: &pb::AuthResult,
        tickets: &mut TicketSlot,
        now: Instant,
    ) -> Vec<PathAction> {
        if self.lost {
            return Vec::new();
        }
        self.touch(now);

        let raw = self.failover.on_auth_result(conn, result, tickets, now);
        let recovered = raw
            .iter()
            .any(|action| matches!(action, FailoverAction::Recovered { .. }));
        let mut actions = self.convert_failover(raw, now);
        if recovered {
            self.promote_pending(conn, now, &mut actions);
        }
        actions
    }

    pub fn on_redial_connected(
        &mut self,
        conn: ConnId,
        sig: PathSig,
        tickets: &TicketSlot,
        now: Instant,
    ) -> Vec<PathAction> {
        if self.lost {
            return Vec::new();
        }
        self.touch(now);
        self.pending = Some((conn, sig));
        let failover_actions = self.failover.on_connected(conn, tickets, now);
        self.convert_failover(failover_actions, now)
    }

    pub fn on_redial_failed(&mut self, tickets: &mut TicketSlot, now: Instant) -> Vec<PathAction> {
        if self.lost {
            return Vec::new();
        }
        self.touch(now);
        let failover_actions = self.failover.on_redial_failed(tickets, now);
        self.convert_failover(failover_actions, now)
    }

    pub fn on_backup_ready(&mut self, conn: ConnId, sig: PathSig, now: Instant) -> Vec<PathAction> {
        if self.lost {
            return Vec::new();
        }
        if self.keeper.on_backup_ready(sig.clone(), now) {
            self.backup = Some(HeldBackup { conn, sig });
            Vec::new()
        } else {
            vec![PathAction::CloseBackup(conn)]
        }
    }

    pub fn on_backup_failed(&mut self, now: Instant) {
        if !self.lost {
            self.keeper.on_acquire_failed(now);
        }
    }

    /// Pauses liveness, backup acquisition, and upgrades. A probe in flight
    /// is dropped and retried no earlier than it was originally due.
    pub fn suspend(&mut self) -> Vec<PathAction> {
        if self.lost {
            return Vec::new();
        }
        if let Some(monitor) = &mut self.monitor {
            monitor.suspend();
        }
        self.keeper.suspend();
        self.suspended = true;
        self.sync_upgrade(self.last_now)
    }

    pub fn resume(&mut self, now: Instant) {
        if self.lost {
            return;
        }
        self.touch(now);
        if let Some(monitor) = &mut self.monitor {
            monitor.resume(now);
        }
        self.keeper.resume(now);
        self.suspended = false;
        // Unblocking never produces an action.
        let _ = self.sync_upgrade(now);
    }

    /// A QUIC-only probe connected; `conn` is its candidate connection.
    pub fn on_probe_ok(&mut self, conn: ConnId, sig: PathSig, now: Instant) -> Vec<PathAction> {
        if self.lost {
            return vec![PathAction::Upgrade(UpgradeAction::DropProbe)];
        }
        self.touch(now);
        let mut actions = self.sync_upgrade(now);
        let raw = self.upgrader.on_probe_ok(now);
        if raw.contains(&UpgradeAction::ResumeOnQuic) {
            self.candidate = Some((conn, sig));
        }
        actions.extend(self.convert_upgrade(raw));
        actions
    }

    pub fn on_probe_failed(&mut self, now: Instant) -> Vec<PathAction> {
        if self.lost {
            return Vec::new();
        }
        self.touch(now);
        let mut actions = self.sync_upgrade(now);
        let raw = self.upgrader.on_probe_failed(now);
        actions.extend(self.convert_upgrade(raw));
        actions
    }

    /// Handles the `AuthResult` read from the candidate's control stream.
    ///
    /// On `Commit` or `CommitNewSession` the candidate becomes the primary
    /// and its new token is stored.
    pub fn on_upgrade_auth_result(
        &mut self,
        conn: ConnId,
        result: &pb::AuthResult,
        tickets: &mut TicketSlot,
        now: Instant,
    ) -> Vec<PathAction> {
        if self.lost {
            return Vec::new();
        }
        self.touch(now);
        let mut actions = self.sync_upgrade(now);
        if self.candidate.as_ref().map(|(id, _)| *id) != Some(conn) {
            return actions;
        }

        let raw = self.upgrader.on_auth_result(result, now);
        let committed = raw.iter().any(|action| {
            matches!(
                action,
                UpgradeAction::Commit | UpgradeAction::CommitNewSession
            )
        });
        if !committed {
            actions.extend(self.convert_upgrade(raw));
            return actions;
        }

        let Some((conn, sig)) = self.candidate.take() else {
            return actions;
        };
        let _ = tickets.store(&result.session_token, self.host_fp);
        self.primary_conn = conn;
        self.primary_sig = sig.clone();
        self.failover.adopt(conn);
        actions.extend(raw.into_iter().map(PathAction::Upgrade));
        let old_backup = self.backup.as_ref().map(|held| held.conn);
        if self.keeper.set_primary(sig, now) {
            if let Some(conn) = old_backup {
                self.backup = None;
                actions.push(PathAction::CloseBackup(conn));
            }
        }
        if let Some(monitor) = &mut self.monitor {
            monitor.reset(now);
        }
        actions.extend(self.restart_upgrade(now));
        actions
    }

    /// Whether the Host replacing the current primary is expected now.
    pub fn expects_replacement(&self) -> bool {
        !self.lost && self.upgrader.expects_replacement()
    }

    pub fn input_verdict(&mut self) -> InputVerdict {
        self.failover.input_verdict()
    }

    pub fn frame_verdict(&mut self, conn: ConnId, keyframe: bool) -> FrameVerdict {
        self.failover.frame_verdict(conn, keyframe)
    }

    pub fn dropped_inputs(&self) -> u64 {
        self.failover.dropped_inputs()
    }

    pub fn is_switching(&self) -> bool {
        matches!(self.failover.phase(), FailoverPhase::Switching { .. })
    }

    pub fn is_lost(&self) -> bool {
        self.lost
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        if self.lost {
            return None;
        }
        let monitor = if self.upgrader.expects_replacement() {
            None
        } else {
            self.monitor.as_ref().and_then(PathMonitor::next_deadline)
        };
        let keeper = if matches!(self.failover.phase(), FailoverPhase::Streaming) {
            self.keeper.next_deadline()
        } else {
            None
        };
        let failover = self.failover.next_deadline();
        let upgrade = self.upgrader.next_deadline();
        [monitor, keeper, failover, upgrade]
            .into_iter()
            .flatten()
            .min()
    }

    fn take_backup_for_failover(&mut self, now: Instant) -> Option<(ConnId, PathSig)> {
        let conn = self.backup.as_ref()?.conn;
        let sig = self.keeper.take_for_failover(now)?;
        self.backup = None;
        self.pending = Some((conn, sig.clone()));
        Some((conn, sig))
    }

    fn promote_pending(&mut self, conn: ConnId, now: Instant, actions: &mut Vec<PathAction>) {
        let Some((pending_conn, sig)) = self.pending.take() else {
            return;
        };
        if pending_conn != conn {
            self.pending = Some((pending_conn, sig));
            return;
        }

        self.primary_conn = pending_conn;
        self.primary_sig = sig.clone();
        let old_backup = self.backup.as_ref().map(|held| held.conn);
        if self.keeper.set_primary(sig, now) {
            if let Some(conn) = old_backup {
                self.backup = None;
                actions.push(PathAction::CloseBackup(conn));
            }
        }
        if let Some(monitor) = &mut self.monitor {
            monitor.reset(now);
        }
        actions.extend(self.restart_upgrade(now));
    }

    fn touch(&mut self, now: Instant) {
        self.last_now = self.last_now.max(now);
    }

    /// Upgrades run only while streaming on an unsuspended primary.
    fn sync_upgrade(&mut self, now: Instant) -> Vec<PathAction> {
        let blocked = self.lost
            || self.suspended
            || self.monitor.is_none()
            || !matches!(self.failover.phase(), FailoverPhase::Streaming);
        if blocked == self.upgrade_blocked {
            return Vec::new();
        }
        self.upgrade_blocked = blocked;
        let raw = self.upgrader.set_blocked(blocked, now);
        self.convert_upgrade(raw)
    }

    /// A new primary restarts the upgrade schedule: QUIC stops it, TCP waits
    /// a full interval again.
    fn restart_upgrade(&mut self, now: Instant) -> Vec<PathAction> {
        let mut raw = self.upgrader.on_primary(TransportKind::Quic, now);
        if self.primary_sig.transport == TransportKind::Tcp {
            raw.extend(self.upgrader.on_primary(TransportKind::Tcp, now));
        }
        self.convert_upgrade(raw)
    }

    fn convert_upgrade(&mut self, raw: Vec<UpgradeAction>) -> Vec<PathAction> {
        if raw.contains(&UpgradeAction::DropProbe) {
            self.candidate = None;
        }
        raw.into_iter().map(PathAction::Upgrade).collect()
    }

    fn convert_failover(&mut self, raw: Vec<FailoverAction>, now: Instant) -> Vec<PathAction> {
        let mut actions = Vec::with_capacity(raw.len());
        let mut lost_emitted = false;
        for action in raw {
            match action {
                FailoverAction::Redial { .. } if !self.cfg.redial => {
                    if !self.lost {
                        tracing::info!(event = "path.lost", reason = "redial_disabled");
                        self.lost = true;
                    }
                    if !lost_emitted {
                        actions.push(PathAction::Lost);
                        lost_emitted = true;
                    }
                }
                FailoverAction::CloseConn(conn)
                    if self.backup.as_ref().is_some_and(|held| held.conn == conn) =>
                {
                    self.backup = None;
                    self.keeper.on_backup_lost(now);
                    actions.push(PathAction::Failover(FailoverAction::CloseConn(conn)));
                }
                other => actions.push(PathAction::Failover(other)),
            }
        }
        actions.extend(self.sync_upgrade(now));
        actions
    }
}
