//! 主路判死与备用连接保活。
//!
//! 本模块是纯逻辑：时间由调用方传入，不访问网络、不读取时钟、不创建定时器。
//!
//! # 判死口径
//!
//! 03 §10.4 只规定了「1.5 秒没有收到任何数据报且 Ping 无响应」。画面静止时
//! 不会有视频数据报，因此静默 [`PROBE_AFTER`] 后由 [`MonitorPoll::send_probe`]
//! 要求调用方补发探测 Ping；调用方必须在发出后调用
//! [`PathMonitor::on_ping_sent`]。周期 Ping（03 §9）同样要调用它，两者在判死
//! 逻辑中是同一种消息。
//!
//! 这里把「任何入站控制消息（含 Pong、HostStats）」都算作路径仍然活着，这是
//! 对「Ping 无响应」的解释（协议文档未进一步规定）。QUIC 传输层心跳是空
//! DATAGRAM 且由传输层内部消费，不会传入会话层，因此不能算作入站数据报。
//!
//! [`PROBE_AFTER`]、[`PROBE_EVERY`]、[`HARD_DEAD_AFTER`]、备用重试间隔和
//! [`ACQUIRE_TIMEOUT`] 是文档未规定的实现默认值。探测 Ping 不会无限等待：
//! 调用方若完全没能发出 Ping，静默达到 [`HARD_DEAD_AFTER`] 时仍会判死。

use std::{collections::VecDeque, time::Duration};

use rsp_transport::{dial::PathKind, TransportKind};
use tokio::time::Instant;

/// 03 §10.4：静默 1.5 秒且至少一个探测 Ping 未答即判死。
pub const DEAD_AFTER: Duration = Duration::from_millis(1500);
/// 文档未规定：静默 500 ms 起补发探测 Ping。
pub const PROBE_AFTER: Duration = Duration::from_millis(500);
/// 文档未规定：探测 Ping 的补发间隔。
pub const PROBE_EVERY: Duration = Duration::from_millis(250);
/// 文档未规定：调用方没能发出 Ping 时的兜底判死时间。
pub const HARD_DEAD_AFTER: Duration = Duration::from_secs(3);
/// 未答 Ping 记录的上限；超过时丢弃最旧的记录。
pub const MAX_OUTSTANDING_PINGS: usize = 32;

/// 判死阈值配置。默认值即上述常量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorConfig {
    pub dead_after: Duration,
    pub probe_after: Duration,
    pub probe_every: Duration,
    pub hard_dead_after: Duration,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            dead_after: DEAD_AFTER,
            probe_after: PROBE_AFTER,
            probe_every: PROBE_EVERY,
            hard_dead_after: HARD_DEAD_AFTER,
        }
    }
}

/// 主路当前的活动状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Suspect,
    Dead { silent_for: Duration },
}

/// 一次 [`PathMonitor::poll`] 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorPoll {
    pub liveness: Liveness,
    /// 调用方应立即发送探测 Ping，并在发出后调用 `on_ping_sent`。
    pub send_probe: bool,
}

/// 会话期间的主路活动监视器。
///
/// 所有时刻都来自调用方；时钟回拨按「没有经过时间」处理，不会让阈值倒退。
#[derive(Debug, Clone)]
pub struct PathMonitor {
    cfg: MonitorConfig,
    last_inbound: Instant,
    last_probe_sent: Option<Instant>,
    outstanding: VecDeque<OutstandingPing>,
    inbound_generation: u64,
    reported_suspect: bool,
    suspended: bool,
    dead: Option<DeadState>,
}

#[derive(Debug, Clone, Copy)]
struct OutstandingPing {
    t1_us: u64,
    generation: u64,
}

#[derive(Debug, Clone, Copy)]
struct DeadState {
    silent_for: Duration,
}

impl PathMonitor {
    /// 进入 Streaming 时创建；`now` 视为最后一次入站。
    pub fn new(cfg: MonitorConfig, now: Instant) -> Self {
        Self {
            cfg,
            last_inbound: now,
            last_probe_sent: None,
            outstanding: VecDeque::new(),
            inbound_generation: 0,
            reported_suspect: false,
            suspended: false,
            dead: None,
        }
    }

    /// 任何数据报（视频/指针/探测）或任何控制消息到达。
    pub fn on_inbound(&mut self, now: Instant) {
        if self.dead.is_some() {
            // Dead 锁存；入站不能清除判死，也不能让 silent_for 倒退。
            return;
        }
        self.last_inbound = self.last_inbound.max(now);
        self.last_probe_sent = None;
        self.inbound_generation = self.inbound_generation.wrapping_add(1);
        self.reported_suspect = false;
    }

    /// 记录一次已发出的 Ping。`t1_us` 会被 `Pong` 用来匹配。
    pub fn on_ping_sent(&mut self, t1_us: u64, now: Instant) {
        let now = self.clamp_now(now);
        self.last_probe_sent = Some(now);
        while self.outstanding.len() >= MAX_OUTSTANDING_PINGS {
            self.outstanding.pop_front();
        }
        self.outstanding.push_back(OutstandingPing {
            t1_us,
            generation: self.inbound_generation,
        });
    }

    /// 收到 Pong；等价于一次入站，并清掉 `t1 <= t1_us` 的未答 Ping。
    pub fn on_pong(&mut self, t1_us: u64, now: Instant) {
        self.on_inbound(now);
        self.outstanding.retain(|ping| ping.t1_us > t1_us);
    }

    /// 后台 PauseStream 期间暂停判死与探测。
    pub fn suspend(&mut self) {
        self.suspended = true;
    }

    /// 恢复前台；`now` 视为最后一次入站，清空未答 Ping 与 Dead 锁存。
    pub fn resume(&mut self, now: Instant) {
        self.suspended = false;
        self.reset(now);
    }

    /// 换路成功后在新的主路上重新开始；不改变 suspend 状态。
    pub fn reset(&mut self, now: Instant) {
        self.last_inbound = now;
        self.last_probe_sent = None;
        self.outstanding.clear();
        self.reported_suspect = false;
        self.dead = None;
    }

    /// 根据调用方传入的时刻返回当前活动状态和是否需要补发探测 Ping。
    ///
    /// `poll` 不记录「已发 Ping」；调用方发出后应调用 [`Self::on_ping_sent`]。
    pub fn poll(&mut self, now: Instant) -> MonitorPoll {
        if self.suspended {
            return MonitorPoll {
                liveness: Liveness::Alive,
                send_probe: false,
            };
        }

        let silence = self.silence_at(now);
        if let Some(dead) = self.dead {
            return MonitorPoll {
                liveness: Liveness::Dead {
                    silent_for: dead.silent_for.max(silence),
                },
                send_probe: false,
            };
        }

        let has_current_outstanding = self
            .outstanding
            .iter()
            .any(|ping| ping.generation == self.inbound_generation);
        if silence >= self.cfg.hard_dead_after
            || (silence >= self.cfg.dead_after && has_current_outstanding)
        {
            self.dead = Some(DeadState {
                silent_for: silence,
            });
            tracing::warn!(
                silent_ms = silence.as_millis(),
                outstanding = self.outstanding.len(),
                "primary path declared dead"
            );
            return MonitorPoll {
                liveness: Liveness::Dead {
                    silent_for: silence,
                },
                send_probe: false,
            };
        }

        if silence < self.cfg.probe_after {
            self.reported_suspect = false;
            return MonitorPoll {
                liveness: Liveness::Alive,
                send_probe: false,
            };
        }

        let sent_since_inbound = self
            .last_probe_sent
            .is_some_and(|sent| sent >= self.last_inbound);
        let probe_due = !sent_since_inbound
            || self.last_probe_sent.is_none_or(|sent| {
                silence.saturating_sub(self.silence_at(sent)) >= self.cfg.probe_every
            });
        if !self.reported_suspect {
            tracing::debug!(
                silent_ms = silence.as_millis() as u64,
                "primary path became suspect"
            );
            self.reported_suspect = true;
        }

        MonitorPoll {
            liveness: Liveness::Suspect,
            send_probe: probe_due,
        }
    }

    /// 下一次需要 poll 的时刻；暂停或已判死时没有期限。
    pub fn next_deadline(&self) -> Option<Instant> {
        if self.suspended || self.dead.is_some() {
            return None;
        }

        let probe_start = self.last_inbound + self.cfg.probe_after;
        let Some(last_probe_sent) = self.last_probe_sent else {
            return Some(probe_start);
        };

        let probe_deadline = (last_probe_sent + self.cfg.probe_every).max(probe_start);
        let has_current_outstanding = self
            .outstanding
            .iter()
            .any(|ping| ping.generation == self.inbound_generation);
        let dead_deadline = if has_current_outstanding {
            self.last_inbound + self.cfg.dead_after
        } else {
            self.last_inbound + self.cfg.hard_dead_after
        };
        Some(probe_deadline.min(dead_deadline))
    }

    pub fn outstanding_pings(&self) -> usize {
        self.outstanding.len()
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    fn silence_at(&self, now: Instant) -> Duration {
        now.max(self.last_inbound)
            .saturating_duration_since(self.last_inbound)
    }

    fn clamp_now(&self, now: Instant) -> Instant {
        let last_probe = match self.last_probe_sent {
            Some(sent) => sent,
            None => self.last_inbound,
        };
        now.max(last_probe).max(self.last_inbound)
    }
}

/// 文档未规定：补备用的指数退避；耗尽后保持 30 秒。
pub const BACKUP_RETRY: [Duration; 4] = [
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(30),
];

/// 文档未规定：一次备用拨号的超时。
pub const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// 主路或备用路的路径签名。路径一律保存为原始类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSig {
    pub path: PathKind,
    pub transport: TransportKind,
}

impl PathSig {
    pub fn new(path: &PathKind, transport: TransportKind) -> Self {
        Self {
            path: path.original().clone(),
            transport,
        }
    }
}

/// 备用路与主路必须是不同的原始路径类型，或使用不同的传输。
pub fn is_distinct(primary: &PathSig, other: &PathSig) -> bool {
    primary.path.original() != other.path.original() || primary.transport != other.transport
}

/// 备用连接的当前状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupState {
    Held(PathSig),
    Missing { next_try: Instant, failures: u32 },
    Acquiring { since: Instant, failures: u32 },
    Suspended,
}

/// 调用方需要执行的备用连接动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupAction {
    /// 拨一条与 `exclude` 不同类型的连接；完成后不要打开控制流。
    Acquire { exclude: PathSig },
}

/// 跟踪一条已握手备用连接的纯逻辑状态机。
#[derive(Debug, Clone)]
pub struct BackupKeeper {
    primary: PathSig,
    state: BackupState,
    before_suspend: Option<BackupState>,
}

impl BackupKeeper {
    pub fn new(primary: PathSig, backup: Option<PathSig>, now: Instant) -> Self {
        let state = backup.map_or_else(
            || BackupState::Missing {
                next_try: now + BACKUP_RETRY[0],
                failures: 0,
            },
            BackupState::Held,
        );
        Self {
            primary,
            state,
            before_suspend: None,
        }
    }

    /// 更新主路。返回 `true` 表示现有备用不再与主路 distinct，调用方应关闭它。
    pub fn set_primary(&mut self, primary: PathSig, now: Instant) -> bool {
        self.primary = primary;

        let should_close = match &self.state {
            BackupState::Held(backup) => !is_distinct(&self.primary, backup),
            _ => false,
        };
        if should_close {
            self.state = BackupState::Missing {
                next_try: now,
                failures: 0,
            };
            tracing::debug!("backup missing after primary change");
        }
        should_close
    }

    /// 记录备用连接已握手完成。不 distinct 时调用方应关掉它，并按失败处理。
    pub fn on_backup_ready(&mut self, sig: PathSig, now: Instant) -> bool {
        if !is_distinct(&self.primary, &sig) {
            self.on_acquire_failed(now);
            return false;
        }

        self.state = BackupState::Held(sig);
        self.before_suspend = None;
        tracing::debug!("backup held");
        true
    }

    /// 记录一次备用拨号失败，并按退避安排下一次尝试。
    pub fn on_acquire_failed(&mut self, now: Instant) {
        let failures = self.failures().saturating_add(1);
        let index = failures.min(BACKUP_RETRY.len() as u32 - 1) as usize;
        self.state = BackupState::Missing {
            next_try: now + BACKUP_RETRY[index],
            failures,
        };
        tracing::debug!(
            failures,
            retry_ms = BACKUP_RETRY[index].as_millis() as u64,
            "backup missing"
        );
    }

    /// 备用连接断开。仅持有时立即安排补一条。
    pub fn on_backup_lost(&mut self, now: Instant) {
        if matches!(self.state, BackupState::Held(_)) {
            self.state = BackupState::Missing {
                next_try: now,
                failures: 0,
            };
            tracing::debug!("backup lost");
        }
    }

    /// 换路取用备用：返回签名并把状态变为 5 秒后重试；没有备用时返回 `None`。
    pub fn take_for_failover(&mut self, now: Instant) -> Option<PathSig> {
        let BackupState::Held(sig) = &self.state else {
            return None;
        };
        let sig = sig.clone();
        self.state = BackupState::Missing {
            next_try: now + BACKUP_RETRY[0],
            failures: 0,
        };
        tracing::debug!("backup consumed for failover");
        Some(sig)
    }

    /// 后台期间不补备用，并记住之前的状态。
    pub fn suspend(&mut self) {
        if matches!(self.state, BackupState::Suspended) {
            return;
        }
        self.before_suspend = Some(self.state.clone());
        self.state = BackupState::Suspended;
    }

    /// 恢复前台；之前持有备用则继续持有，否则立即安排一次补发。
    pub fn resume(&mut self, now: Instant) {
        let previous = self.before_suspend.take();
        self.state = match previous.as_ref() {
            Some(BackupState::Held(sig)) => BackupState::Held(sig.clone()),
            _ => BackupState::Missing {
                next_try: now,
                failures: 0,
            },
        };
        tracing::debug!(held = self.has_backup(), "backup keeper resumed");
    }

    /// 到期待补时返回拨号动作并进入 `Acquiring`；Acquiring 超时按失败处理。
    pub fn poll(&mut self, now: Instant) -> Option<BackupAction> {
        match self.state.clone() {
            BackupState::Suspended | BackupState::Held(_) => None,
            BackupState::Missing { next_try, failures } => {
                if now < next_try {
                    return None;
                }
                self.state = BackupState::Acquiring {
                    since: now,
                    failures,
                };
                tracing::debug!(%failures, "acquiring backup path");
                Some(BackupAction::Acquire {
                    exclude: self.primary.clone(),
                })
            }
            BackupState::Acquiring { since, failures } => {
                if now.saturating_duration_since(since) < ACQUIRE_TIMEOUT {
                    return None;
                }
                self.on_acquire_failed(now);
                if let BackupState::Missing {
                    failures: next_failures,
                    ..
                } = &self.state
                {
                    tracing::debug!(
                        failures = *next_failures,
                        previous_failures = failures,
                        "backup acquisition timed out"
                    );
                }
                None
            }
        }
    }

    pub fn state(&self) -> &BackupState {
        &self.state
    }

    pub fn has_backup(&self) -> bool {
        matches!(self.state, BackupState::Held(_))
    }

    /// 下一次需要 poll 的时刻；持有备用或暂停时没有期限。
    pub fn next_deadline(&self) -> Option<Instant> {
        match &self.state {
            BackupState::Missing { next_try, .. } => Some(*next_try),
            BackupState::Acquiring { since, .. } => Some(*since + ACQUIRE_TIMEOUT),
            BackupState::Held(_) | BackupState::Suspended => None,
        }
    }

    fn failures(&self) -> u32 {
        match &self.state {
            BackupState::Missing { failures, .. } | BackupState::Acquiring { failures, .. } => {
                *failures
            }
            BackupState::Held(_) | BackupState::Suspended => 0,
        }
    }
}
