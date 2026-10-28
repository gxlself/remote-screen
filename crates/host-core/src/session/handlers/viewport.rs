//! Host-side viewport reporting and crop/ROI policy wiring (03 §5.5).

use super::{rate::RatePlan, stream, stream::StreamTarget, ConnCx};
use crate::{
    auth,
    pipeline::{capture, viewport, viewport_geom, CropPlan},
    platform::{PixelRect, Platform},
    session::manager::{EndReason, HostServices, SessionManager},
};
use rsp_core::{features, pb};
use std::sync::Arc;

pub struct Host;
impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self
    }
}

#[derive(Default)]
pub struct Conn {
    pub policy: Option<viewport::ViewportPolicy>,
    pub policy_display: Option<(u32, u32, u32)>,
    pub last_seq: u32,
    pub crop_disabled: bool,
}

#[derive(Debug)]
pub enum Event {}

pub const FEATURES: u64 = features::VIEWPORT;

pub fn on_viewport<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::Viewport) {
    if cx.info.client_features & features::VIEWPORT == 0
        || cx.info.intent == pb::Intent::Files
        || m.seq <= cx.st.viewport.last_seq
    {
        return;
    }
    cx.st.viewport.last_seq = m.seq;

    let Some(target) = stream::current_target(cx) else {
        send_ack(cx, m.seq);
        return;
    };
    if m.display_id != target.wire_id {
        send_ack(cx, m.seq);
        return;
    }

    let display = (
        target.display.id,
        target.display.width_px,
        target.display.height_px,
    );
    if cx.st.viewport.policy_display != Some(display) || cx.st.viewport.policy.is_none() {
        let caps = viewport::ViewportCaps {
            crop: cx.platform().capture_region_supported() && !cx.st.viewport.crop_disabled,
            roi: cx
                .st
                .stream
                .pipeline
                .as_ref()
                .is_some_and(|pipeline| pipeline.roi_supported()),
        };
        cx.st.viewport.policy = Some(viewport::ViewportPolicy::new(
            target.display.width_px,
            target.display.height_px,
            caps,
        ));
        cx.st.viewport.policy_display = Some(display);
    }

    let rect = normalize_rect(m.rect, (target.display.width_px, target.display.height_px));
    let report = viewport::ViewReport {
        seq: m.seq,
        rect,
        settled: m.settled,
        view_w: m.view_width_px,
        view_h: m.view_height_px,
        crop_ok: m.crop_ok,
    };
    let now = cx.now_us();
    let output = cx
        .st
        .viewport
        .policy
        .as_mut()
        .map_or_else(viewport::PolicyOutput::default, |policy| {
            policy.on_report(now, report)
        });
    apply(cx, output);
    send_ack(cx, m.seq);
}

fn normalize_rect(rect: Option<pb::PixelRect>, display: (u32, u32)) -> Option<PixelRect> {
    let rect = rect?;
    if rect.w == 0 || rect.h == 0 || rect.x >= display.0 || rect.y >= display.1 {
        return None;
    }
    let w = rect.w.min(display.0 - rect.x);
    let h = rect.h.min(display.1 - rect.y);
    (w >= 2 && h >= 2).then_some(PixelRect {
        x: rect.x,
        y: rect.y,
        w,
        h,
    })
}

fn send_ack<P: Platform>(cx: &mut ConnCx<'_, P>, seq: u32) {
    let mode = cx
        .st
        .viewport
        .policy
        .as_ref()
        .map_or(viewport::ViewMode::Off, viewport::ViewportPolicy::mode);
    cx.send(pb::ViewportAck {
        seq,
        mode: mode_to_pb(mode) as i32,
    });
}

fn mode_to_pb(mode: viewport::ViewMode) -> pb::viewport_ack::Mode {
    match mode {
        viewport::ViewMode::Off => pb::viewport_ack::Mode::Off,
        viewport::ViewMode::Roi => pb::viewport_ack::Mode::Roi,
        viewport::ViewMode::Crop => pb::viewport_ack::Mode::Crop,
    }
}

fn apply<P: Platform>(cx: &mut ConnCx<'_, P>, output: viewport::PolicyOutput) {
    if output.crop.is_some() {
        if let Some(target) = stream::current_target(cx) {
            stream::rebuild(cx, target);
        }
        return;
    }
    if let Some(roi) = output.roi {
        apply_roi(cx, roi);
    }
}

fn apply_roi<P: Platform>(cx: &mut ConnCx<'_, P>, focus: Option<PixelRect>) {
    let Some(focus) = focus else {
        if let Some(pipeline) = &cx.st.stream.pipeline {
            pipeline.set_roi(Vec::new());
        }
        return;
    };
    let Some(target) = stream::current_target(cx) else {
        return;
    };
    let Some(pipeline) = &cx.st.stream.pipeline else {
        return;
    };
    let viewport = cx
        .st
        .viewport
        .policy
        .as_ref()
        .and_then(|policy| policy.last_report())
        .and_then(|report| report.rect)
        .unwrap_or(PixelRect {
            x: 0,
            y: 0,
            w: target.display.width_px,
            h: target.display.height_px,
        });
    pipeline.set_roi(viewport_geom::focus_regions(
        focus,
        viewport,
        (target.display.width_px, target.display.height_px),
        (pipeline.width, pipeline.height),
    ));
}

pub fn on_host_start<P: Platform>(_mgr: &Arc<SessionManager<P>>) {}
pub fn on_admitted<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}

pub fn on_stream_started<P: Platform>(cx: &mut ConnCx<'_, P>) {
    if stream::current_target(cx).is_none() || cx.st.stream.pipeline.is_none() {
        return;
    }
    let roi_supported = cx
        .st
        .stream
        .pipeline
        .as_ref()
        .is_some_and(|pipeline| pipeline.roi_supported());
    let now = cx.now_us();
    let crop_supported = cx.platform().capture_region_supported();
    let crop_disabled = cx.st.viewport.crop_disabled;
    let output =
        cx.st
            .viewport
            .policy
            .as_mut()
            .map_or_else(viewport::PolicyOutput::default, |policy| {
                policy.set_caps(
                    now,
                    viewport::ViewportCaps {
                        crop: crop_supported && !crop_disabled,
                        roi: roi_supported,
                    },
                )
            });
    apply(cx, output);
    if let Some(focus) = cx
        .st
        .viewport
        .policy
        .as_ref()
        .and_then(|policy| policy.roi())
    {
        apply_roi(cx, Some(focus));
    }
}

pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_event<P: Platform>(_cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {}
}

pub fn on_tick<P: Platform>(cx: &mut ConnCx<'_, P>, now_us: u64) {
    let output = cx
        .st
        .viewport
        .policy
        .as_mut()
        .map_or_else(viewport::PolicyOutput::default, |policy| {
            policy.poll(now_us)
        });
    apply(cx, output);
}

pub fn deadline<P: Platform>(cx: &ConnCx<'_, P>) -> Option<u64> {
    cx.st
        .viewport
        .policy
        .as_ref()
        .and_then(viewport::ViewportPolicy::deadline)
}

pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    cx.st.viewport = Conn::default();
}

/// Return the crop to use for the next pipeline start. A stale policy is
/// cleared when the selected display no longer matches it.
pub fn crop_for_start<P: Platform>(
    cx: &mut ConnCx<'_, P>,
    target: &StreamTarget,
    plan: &RatePlan,
    start: &pb::StartStream,
) -> Option<(CropPlan, u32)> {
    let display = (
        target.display.id,
        target.display.width_px,
        target.display.height_px,
    );
    if cx.st.viewport.policy_display != Some(display) {
        cx.st.viewport.policy = None;
        cx.st.viewport.policy_display = None;
        return None;
    }
    let policy = cx.st.viewport.policy.as_ref()?;
    let crop = policy.crop()?;
    let report = policy.last_report()?;
    let viewport = report.rect?;
    let budget = capture::dimensions(
        &target.display,
        plan.max_width,
        if cx.conn.kind() == rsp_transport::TransportKind::Tcp {
            1080
        } else {
            0
        },
    )
    .ok()?;
    let output = viewport_geom::crop_output_size(
        crop,
        viewport,
        budget,
        (report.view_w, report.view_h),
        (start.max_width, start.max_height),
    );
    Some((
        CropPlan {
            rect: crop,
            out_w: output.0,
            out_h: output.1,
        },
        policy.crop_seq(),
    ))
}

/// Disable crop for this connection after a region pipeline failed.
pub fn crop_failed<P: Platform>(cx: &mut ConnCx<'_, P>) {
    cx.st.viewport.crop_disabled = true;
    let now = cx.now_us();
    if let Some(policy) = &mut cx.st.viewport.policy {
        policy.crop_failed(now);
    }
}
