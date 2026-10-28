//! Session-layer wiring for the pure background recovery timeline.

use std::{sync::Arc, time::Duration};

use rsp_client::{
    recovery::{
        background::{BackgroundTimeline, BgAction, BgState},
        failover::{TicketSlot, TOKEN_LEN},
    },
    SessionHandle,
};
use rsp_core::{hid::InputMsg, pb};
use tokio::time::Instant;

/// A wall-clock source represented as a tokio `Instant` for timeline use.
#[derive(Clone)]
pub struct BgClock {
    base: Instant,
    base_us: u64,
    now_us: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl BgClock {
    pub fn new(now_us: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        let base_us = now_us();
        Self {
            base: Instant::now(),
            base_us,
            now_us,
        }
    }

    pub fn now(&self) -> Instant {
        self.base + Duration::from_micros((self.now_us)().saturating_sub(self.base_us))
    }
}

pub trait BgTarget {
    fn release_all(&self);
    fn pause(&self);
    fn resume(&self);
    fn request_resume_keyframe(&self);
    fn close_background(&self);
    fn resume_token(&self) -> Option<Vec<u8>>;
}

impl BgTarget for SessionHandle {
    fn release_all(&self) {
        if let Err(error) = self.send_input(InputMsg::ReleaseAll) {
            tracing::debug!(%error, "failed to release input while entering background");
        }
    }

    fn pause(&self) {
        SessionHandle::pause(self);
    }

    fn resume(&self) {
        SessionHandle::resume(self);
    }

    fn request_resume_keyframe(&self) {
        self.request_keyframe_for(pb::keyframe_request::Reason::Resume);
    }

    fn close_background(&self) {
        self.close_with(pb::goodbye::Reason::Background);
    }

    fn resume_token(&self) -> Option<Vec<u8>> {
        SessionHandle::resume_token(self)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BgFlow {
    Continue,
    Closed,
    Resume { token: [u8; TOKEN_LEN] },
    ReturnToDevices,
}

pub struct BgDriver {
    timeline: BackgroundTimeline,
    ticket: TicketSlot,
    host_fp: [u8; 32],
}

impl BgDriver {
    pub fn new(grace: Option<Duration>, host_fp: [u8; 32]) -> Self {
        Self {
            timeline: BackgroundTimeline::with_grace(grace),
            ticket: TicketSlot::new(),
            host_fp,
        }
    }

    pub fn set_background(
        &mut self,
        on: bool,
        now: Instant,
        target: Option<&dyn BgTarget>,
    ) -> BgFlow {
        let actions = self.timeline.set_background(on, now);
        self.apply(actions, now, target)
    }

    pub fn poll(&mut self, now: Instant, target: Option<&dyn BgTarget>) -> BgFlow {
        let actions = self.timeline.poll(now);
        self.apply(actions, now, target)
    }

    pub fn on_connection_lost(&mut self, now: Instant, target: Option<&dyn BgTarget>) -> BgFlow {
        let actions = self.timeline.on_connection_lost(now);
        let flow = self.apply(actions, now, target);
        if matches!(self.timeline.state(), BgState::Closed { .. }) {
            merge_flow(flow, BgFlow::Closed)
        } else {
            flow
        }
    }

    pub fn on_keyframe(&mut self, now: Instant) {
        self.timeline.on_keyframe(now);
    }

    pub fn set_grace(&mut self, grace: Option<Duration>, now: Instant) {
        if self.timeline.grace() != grace {
            self.timeline.set_grace(grace, now);
        }
    }

    pub fn state(&self) -> BgState {
        self.timeline.state()
    }

    pub fn next_wake(&self, now: Instant) -> Option<Instant> {
        if matches!(self.timeline.state(), BgState::Foreground) {
            self.timeline.next_deadline()
        } else {
            let tick = now + Duration::from_secs(1);
            Some(
                self.timeline
                    .next_deadline()
                    .map_or(tick, |deadline| deadline.min(tick)),
            )
        }
    }

    fn apply(
        &mut self,
        actions: Vec<BgAction>,
        now: Instant,
        target: Option<&dyn BgTarget>,
    ) -> BgFlow {
        let mut flow = BgFlow::Continue;
        let mut close_requested = false;
        for action in actions {
            match action {
                BgAction::SendReleaseAll => {
                    if let Some(target) = target {
                        target.release_all();
                    } else {
                        tracing::debug!("background release skipped without a connection");
                    }
                }
                BgAction::SendPauseStream => {
                    if let Some(target) = target {
                        target.pause();
                    } else {
                        tracing::debug!("background pause skipped without a connection");
                    }
                }
                BgAction::SendResumeStream => {
                    if let Some(target) = target {
                        target.resume();
                    } else {
                        tracing::debug!("background resume skipped without a connection");
                    }
                }
                BgAction::RequestKeyframe => {
                    if let Some(target) = target {
                        target.request_resume_keyframe();
                    } else {
                        tracing::debug!("background keyframe request skipped without a connection");
                    }
                }
                BgAction::SuspendMonitors | BgAction::ResumeMonitors => {
                    tracing::debug!(?action, "background monitor action");
                }
                BgAction::SendGoodbyeBackground | BgAction::CloseConnection => {
                    if !close_requested {
                        close_requested = true;
                        if let Some(target) = target {
                            target.close_background();
                        } else {
                            tracing::debug!("background close skipped without a connection");
                        }
                    }
                    flow = merge_flow(flow, BgFlow::Closed);
                }
                BgAction::MarkTicketLost { at } => {
                    self.ticket.clear();
                    if let Some(target) = target {
                        if let Some(token) = target.resume_token() {
                            if !self.ticket.store(&token, self.host_fp) {
                                tracing::warn!("invalid background resume token");
                            } else {
                                self.ticket.mark_lost(at);
                            }
                        } else {
                            tracing::warn!("background connection has no resume token");
                        }
                    } else {
                        tracing::debug!("background ticket mark skipped without a connection");
                    }
                }
                BgAction::StartResume => {
                    if let Some(token) = self.ticket.token_for(&self.host_fp, now) {
                        self.ticket.clear();
                        flow = merge_flow(flow, BgFlow::Resume { token });
                    } else {
                        self.ticket.clear();
                        flow = merge_flow(flow, BgFlow::ReturnToDevices);
                    }
                }
                BgAction::ExpireTicket => self.ticket.clear(),
                BgAction::ReturnToDevices => flow = merge_flow(flow, BgFlow::ReturnToDevices),
            }
        }
        flow
    }
}

fn merge_flow(current: BgFlow, next: BgFlow) -> BgFlow {
    fn rank(flow: &BgFlow) -> u8 {
        match flow {
            BgFlow::Continue => 0,
            BgFlow::Closed => 1,
            BgFlow::ReturnToDevices | BgFlow::Resume { .. } => 2,
        }
    }
    if rank(&next) >= rank(&current) {
        next
    } else {
        current
    }
}
