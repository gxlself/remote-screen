//! 文件传输的进度、续传与取消状态机。
//!
//! This module is deliberately free of I/O and timers. Callers pass unix
//! microseconds to the methods that need a clock, which makes the state
//! machine deterministic and easy to share between host and app code.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Whether this side is sending or receiving the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direction {
    Send,
    Receive,
}

/// Lifecycle state of one file transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferState {
    Queued,
    Running,
    Interrupted,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

/// Stable reason for a failed transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailReason {
    Io,
    Sha256Mismatch,
    ResumeMismatch,
    SourceChanged,
    Protocol,
    TooLarge,
}

/// A log-safe snapshot of a transfer's current progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferProgress {
    pub transfer_id: String,
    pub direction: Direction,
    pub state: TransferState,
    pub done: u64,
    pub size: u64,
    pub bytes_per_sec: u64,
    pub fail: Option<FailReason>,
}

/// Errors returned by transfer state transitions.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    #[error("invalid transition from {from:?} during {op}")]
    InvalidTransition {
        from: TransferState,
        op: &'static str,
    },
    #[error("advancing {done} by {add} bytes exceeds size {size}")]
    Overflow { done: u64, add: u64, size: u64 },
    #[error("resume offset {offset} exceeds size {size}")]
    OffsetBeyondSize { offset: u64, size: u64 },
}

/// State machine for one incoming or outgoing file transfer.
pub struct Transfer {
    transfer_id: String,
    direction: Direction,
    state: TransferState,
    done: u64,
    size: u64,
    bytes_per_sec: u64,
    fail: Option<FailReason>,
    last_event_us: Option<u64>,
    rate_started_us: u64,
    rate_started_done: u64,
}

impl Transfer {
    /// Creates a queued transfer that has not moved any bytes yet.
    pub fn new(transfer_id: String, direction: Direction, size: u64) -> Self {
        Self {
            transfer_id,
            direction,
            state: TransferState::Queued,
            done: 0,
            size,
            bytes_per_sec: 0,
            fail: None,
            last_event_us: None,
            rate_started_us: 0,
            rate_started_done: 0,
        }
    }

    /// Starts a new transfer or resumes one at `offset`.
    pub fn start(&mut self, offset: u64, now_us: u64) -> Result<TransferProgress, TransferError> {
        if !matches!(
            &self.state,
            TransferState::Queued | TransferState::Interrupted
        ) {
            return Err(self.invalid_transition("start"));
        }
        if offset > self.size {
            return Err(TransferError::OffsetBeyondSize {
                offset,
                size: self.size,
            });
        }

        self.state = TransferState::Running;
        self.done = offset;
        self.fail = None;
        self.last_event_us = Some(now_us);
        self.rate_started_us = now_us;
        self.rate_started_done = offset;
        self.bytes_per_sec = 0;
        Ok(self.progress())
    }

    /// Adds `n` transferred bytes and returns a throttled progress event.
    ///
    /// Events are emitted once per 100 ms, and always when the transfer
    /// reaches its declared size.
    pub fn advance(
        &mut self,
        n: u64,
        now_us: u64,
    ) -> Result<Option<TransferProgress>, TransferError> {
        if !matches!(self.state, TransferState::Running) {
            return Err(self.invalid_transition("advance"));
        }
        if n > self.size - self.done {
            return Err(TransferError::Overflow {
                done: self.done,
                add: n,
                size: self.size,
            });
        }

        self.done += n;
        self.update_rate(now_us);

        let event_due = self
            .last_event_us
            .is_none_or(|last| now_us.saturating_sub(last) >= 100_000);
        if !event_due && self.done != self.size {
            return Ok(None);
        }

        self.last_event_us = Some(now_us);
        Ok(Some(self.progress()))
    }

    /// Pauses a running transfer while retaining the bytes received so far.
    pub fn interrupt(&mut self, now_us: u64) -> Result<TransferProgress, TransferError> {
        if !matches!(self.state, TransferState::Running) {
            return Err(self.invalid_transition("interrupt"));
        }

        self.state = TransferState::Interrupted;
        self.last_event_us = Some(now_us);
        Ok(self.progress())
    }

    /// Enters verification after all bytes have been transferred.
    pub fn verifying(&mut self) -> Result<TransferProgress, TransferError> {
        if !matches!(self.state, TransferState::Running) || self.done != self.size {
            return Err(self.invalid_transition("verifying"));
        }

        self.state = TransferState::Verifying;
        Ok(self.progress())
    }

    /// Marks a verified transfer as complete.
    pub fn complete(&mut self) -> Result<TransferProgress, TransferError> {
        if !matches!(self.state, TransferState::Verifying) {
            return Err(self.invalid_transition("complete"));
        }

        self.state = TransferState::Completed;
        Ok(self.progress())
    }

    /// Fails any non-terminal transfer with a stable reason.
    pub fn fail(&mut self, reason: FailReason) -> Result<TransferProgress, TransferError> {
        if self.state_is_terminal() {
            return Err(self.invalid_transition("fail"));
        }

        self.state = TransferState::Failed;
        self.fail = Some(reason);
        Ok(self.progress())
    }

    /// Cancels any non-terminal transfer.
    pub fn cancel(&mut self) -> Result<TransferProgress, TransferError> {
        if self.state_is_terminal() {
            return Err(self.invalid_transition("cancel"));
        }

        self.state = TransferState::Cancelled;
        Ok(self.progress())
    }

    /// Returns a snapshot without changing the transfer.
    pub fn progress(&self) -> TransferProgress {
        TransferProgress {
            transfer_id: self.transfer_id.clone(),
            direction: self.direction.clone(),
            state: self.state.clone(),
            done: self.done,
            size: self.size,
            bytes_per_sec: self.bytes_per_sec,
            fail: self.fail.clone(),
        }
    }

    fn invalid_transition(&self, op: &'static str) -> TransferError {
        TransferError::InvalidTransition {
            from: self.state.clone(),
            op,
        }
    }

    fn state_is_terminal(&self) -> bool {
        matches!(
            self.state,
            TransferState::Completed | TransferState::Failed | TransferState::Cancelled
        )
    }

    fn update_rate(&mut self, now_us: u64) {
        let elapsed_us = now_us.saturating_sub(self.rate_started_us);
        if elapsed_us < 1_000_000 {
            return;
        }

        let transferred = u128::from(self.done - self.rate_started_done);
        let rate = transferred * 1_000_000 / u128::from(elapsed_us);
        self.bytes_per_sec = u64::try_from(rate).unwrap_or(u64::MAX);
        self.rate_started_us = now_us;
        self.rate_started_done = self.done;
    }
}

/// Cloneable cancellation bit shared with an I/O engine.
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
