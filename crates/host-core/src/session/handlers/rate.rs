//! Per-connection rate control, FEC and host statistics.
use super::{stream, stream::StreamTarget, ConnCx};
use crate::{
    auth,
    pipeline::ratectl::path_of,
    platform::Platform,
    session::{
        displays::fit_stream_size,
        manager::{EndReason, HostServices, SessionManager},
    },
};
use rsp_core::{
    features, pb,
    ratectl::{Controller, Decision},
};
use std::sync::Arc;

const TICK_US: u64 = 1_000_000;

pub struct Host {}
impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self {}
    }
}
#[derive(Default)]
pub struct Conn {
    pub ctl: Option<Controller>,
    pub next_tick_us: u64,
    pub feedback_seen: bool,
    pub stats_on: bool,
}
#[derive(Debug)]
pub enum Event {}
pub const FEATURES: u64 = features::RATE_FEEDBACK;
pub fn on_host_start<P: Platform>(_mgr: &Arc<SessionManager<P>>) {}
pub fn on_admitted<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}
pub fn on_stream_started<P: Platform>(cx: &mut ConnCx<'_, P>) {
    let now = cx.now_us();
    if let (Some(ctl), Some(pipeline)) = (&mut cx.st.rate.ctl, &cx.st.stream.pipeline) {
        let d = for_transport(ctl.tick(now), cx.conn.kind());
        pipeline.set_rate(super::audio::video_kbps(cx, &d), u32::from(pipeline.fps));
        pipeline.set_fec_ratio(d.fec_ratio);
        cx.st.rate.stats_on = true;
        cx.st.rate.next_tick_us = now.saturating_add(TICK_US);
    }
}
pub fn on_stream_stopped<P: Platform>(cx: &mut ConnCx<'_, P>) {
    cx.st.rate.stats_on = false;
}
pub fn on_event<P: Platform>(_cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {}
}
pub fn deadline<P: Platform>(cx: &ConnCx<'_, P>) -> Option<u64> {
    (cx.st.rate.stats_on && cx.st.stream.pipeline.is_some()).then_some(cx.st.rate.next_tick_us)
}
pub fn on_tick<P: Platform>(cx: &mut ConnCx<'_, P>, now_us: u64) {
    if deadline(cx).is_none_or(|at| now_us < at) {
        return;
    }
    if let Some(ctl) = &mut cx.st.rate.ctl {
        let d = for_transport(ctl.tick(now_us), cx.conn.kind());
        apply(cx, d);
    }
    let stats = cx.st.stream.pipeline.as_ref().and_then(|pipeline| {
        pipeline.stats().lock().ok()?.tick(
            // Capture timestamps use the system clock, not the session's offset clock.
            rsp_core::clock::unix_micros(),
            0, // Platform does not expose CPU usage yet.
            cx.info.path,
            cx.info.derp_region.clone(),
        )
    });
    if let Some(stats) = stats {
        cx.send(stats);
    }
    cx.st.rate.next_tick_us = now_us.saturating_add(TICK_US);
}
pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    cx.st.rate = Conn::default();
}

pub struct RatePlan {
    pub bitrate_kbps: u32,
    pub fps: u32,
    pub max_width: u32,
    pub max_height: u32,
}
pub fn initial<P: Platform>(
    cx: &mut ConnCx<'_, P>,
    m: &pb::StartStream,
    t: &StreamTarget,
) -> RatePlan {
    let now = cx.now_us();
    let ctl = cx.st.rate.ctl.get_or_insert_with(|| {
        Controller::new(
            path_of(
                cx.info.path,
                match cx.conn.kind() {
                    rsp_transport::TransportKind::Quic => pb::Transport::Quic,
                    rsp_transport::TransportKind::Tcp => pb::Transport::Tcp,
                },
            ),
            m.quality(),
            m.bitrate_kbps,
            (t.display.width_px, t.display.height_px),
            now,
        )
    });
    let d = for_transport(ctl.tick(now), cx.conn.kind());
    let (width, height) = ctl.stream_size();
    let (max_width, max_height) = fit_stream_size(
        &t.display,
        cap(width, m.max_width),
        cap(height, m.max_height),
    );
    RatePlan {
        bitrate_kbps: super::audio::video_kbps(cx, &d),
        fps: cap(
            cap(d.fps, m.fps),
            cx.mgr.services().config.get().video.max_fps,
        )
        .max(1),
        max_width,
        max_height,
    }
}

fn cap(value: u32, limit: u32) -> u32 {
    if limit == 0 {
        value
    } else {
        value.min(limit)
    }
}

fn apply<P: Platform>(cx: &mut ConnCx<'_, P>, d: Decision) {
    let fps = cap(
        cap(d.fps, cx.st.stream.last_start.map_or(0, |m| m.fps)),
        cx.mgr.services().config.get().video.max_fps,
    )
    .max(1);
    if let Some(pipeline) = &cx.st.stream.pipeline {
        pipeline.set_rate(super::audio::video_kbps(cx, &d), fps);
        pipeline.set_fec_ratio(d.fec_ratio);
    }
    if cx.info.client_features & FEATURES != 0 {
        if let Some(mut update) = d.rate_update {
            update.fps = fps;
            cx.send(update);
        }
    }
    if d.config_changed {
        if let Some(target) = stream::current_target(cx) {
            stream::rebuild(cx, target);
        }
    }
}

pub fn on_feedback<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::Feedback) {
    if m.epoch < cx.st.stream.epoch {
        return;
    }
    let now = cx.now_us();
    if let Some(ctl) = &mut cx.st.rate.ctl {
        let d = for_transport(ctl.on_feedback(&m, now), cx.conn.kind());
        cx.st.rate.feedback_seen = true;
        apply(cx, d);
    }
}
pub fn on_set_quality<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::SetQuality) {
    let now = cx.now_us();
    if let Some(ctl) = &mut cx.st.rate.ctl {
        let d = for_transport(ctl.set_quality(m.quality(), now), cx.conn.kind());
        apply(cx, d);
    }
}

// The shared controller budgets QUIC parity; TCP sends whole frames without FEC.
fn for_transport(mut d: Decision, kind: rsp_transport::TransportKind) -> Decision {
    if kind == rsp_transport::TransportKind::Tcp {
        d.encoder_kbps = d.target_kbps;
        d.fec_ratio = 0.0;
        if let Some(update) = &mut d.rate_update {
            update.fec_ratio_pct = 0;
        }
    }
    d
}
