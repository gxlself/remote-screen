//! Per-connection stream gating and session resume snapshots.
use super::{cursor, displays, rate, ConnCx};
use crate::{
    auth,
    pipeline::VideoPipeline,
    platform::{DisplayInfo, Platform, PlatformError},
    session::{
        manager::{EndReason, HostServices, SessionManager},
        streamgate::{GateAction, StreamGate, MAX_PAUSE_US},
    },
};
use rsp_core::{errors::ErrorCode, features, pb};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

const SAVED_TTL_US: u64 = 60_000_000;
const MAX_SAVED: usize = 32;

struct Saved {
    last_start: pb::StartStream,
    wire_id: u32,
    epoch: u32,
    at_us: u64,
}

pub struct Host {
    saved: Mutex<HashMap<String, Saved>>,
}
impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self {
            saved: Mutex::new(HashMap::new()),
        }
    }
}
pub struct Conn {
    pub codecs: Vec<pb::Codec>,
    pub gate: StreamGate,
    pub pipeline: Option<VideoPipeline>,
    pub target: Option<StreamTarget>,
    pub last_start: Option<pb::StartStream>,
    pub epoch: u32,
}
impl Default for Conn {
    fn default() -> Self {
        Self {
            codecs: Vec::new(),
            gate: StreamGate::new(MAX_PAUSE_US),
            pipeline: None,
            target: None,
            last_start: None,
            epoch: 0,
        }
    }
}
#[derive(Debug)]
pub enum Event {}
pub const FEATURES: u64 = features::HQ_REFRESH;
pub fn on_host_start<P: Platform>(_mgr: &Arc<SessionManager<P>>) {}
pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    if !cx.info.resumed {
        return;
    }
    let saved = {
        let mut saved = cx
            .mgr
            .features()
            .stream
            .saved
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let cutoff = cx.now_us().saturating_sub(SAVED_TTL_US);
        saved.retain(|_, entry| entry.at_us >= cutoff);
        saved.remove(&cx.info.session_id)
    };
    let Some(saved) = saved else {
        return;
    };
    cx.st.stream.gate = StreamGate::resumed(MAX_PAUSE_US).0;
    cx.st.stream.last_start = Some(saved.last_start);
    cx.st.stream.epoch = saved.epoch;
    match displays::resolve_start(cx, saved.wire_id) {
        Ok(target) => {
            cx.st.stream.target = Some(target);
            start_pipeline(cx);
        }
        Err(err) => cx.send(err),
    }
}
pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}
pub fn on_stream_started<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_event<P: Platform>(_cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {}
}
pub fn deadline<P: Platform>(cx: &ConnCx<'_, P>) -> Option<u64> {
    cx.st.stream.gate.next_deadline()
}
pub fn on_tick<P: Platform>(cx: &mut ConnCx<'_, P>, now_us: u64) {
    if cx
        .st
        .stream
        .gate
        .poll(now_us)
        .contains(&GateAction::StartEncoder)
    {
        tracing::info!("暂停兜底超时，自动恢复推流");
        start_pipeline(cx);
    }
}
pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    if let Some(last_start) = cx.st.stream.last_start {
        let now = cx.now_us();
        let mut saved = cx
            .mgr
            .features()
            .stream
            .saved
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let cutoff = now.saturating_sub(SAVED_TTL_US);
        saved.retain(|_, entry| entry.at_us >= cutoff);
        saved.insert(
            cx.info.session_id.clone(),
            Saved {
                last_start,
                wire_id: cx
                    .st
                    .stream
                    .target
                    .as_ref()
                    .map_or(last_start.display_id, |target| target.wire_id),
                epoch: cx.st.stream.epoch,
                at_us: now,
            },
        );
        if saved.len() > MAX_SAVED {
            let oldest = saved
                .iter()
                .min_by_key(|(_, entry)| entry.at_us)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                saved.remove(&oldest);
            }
        }
    }
    drop(cx.st.stream.pipeline.take());
}
#[derive(Clone)]
pub struct StreamTarget {
    pub display: DisplayInfo,
    pub wire_id: u32,
    pub header_id: u8,
}
pub fn on_start_stream<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::StartStream) {
    if cx.info.intent == pb::Intent::Files {
        cx.send(pb::ErrorInfo {
            code: ErrorCode::PolicyDenied.code().into(),
            message: "files session has no video".into(),
            ..Default::default()
        });
        return;
    }
    let prev = cx.st.stream.last_start.replace(m);
    let now = cx.now_us();
    let actions = cx.st.stream.gate.on_start_stream(now);
    if actions.contains(&GateAction::StartEncoder) {
        cx.st.stream.target = None;
        start_pipeline(cx);
    } else if actions.contains(&GateAction::ForceKeyframe) {
        if prev == Some(m) {
            on_keyframe_request(cx, pb::KeyframeRequest::default());
        } else {
            // A changed StartStream rebuilds the encoder and advances epoch.
            drop(cx.st.stream.pipeline.take());
            cx.st.stream.target = None;
            start_pipeline(cx);
        }
    }
}
fn start_pipeline<P: Platform>(cx: &mut ConnCx<'_, P>) -> bool {
    let Some(start) = cx.st.stream.last_start else {
        return false;
    };
    let target = match cx.st.stream.target.clone() {
        Some(target) => target,
        None => match displays::resolve_start(cx, start.display_id) {
            Ok(target) => target,
            Err(err) => {
                cx.send(err);
                return false;
            }
        },
    };
    let plan = rate::initial(cx, &start, &target);
    let crop = super::viewport::crop_for_start(cx, &target, &plan, &start);
    let crop_seq = crop.map(|(_, seq)| seq).unwrap_or(0);
    let epoch = cx.st.stream.epoch.wrapping_add(1);
    let crop_plan = crop.map(|(plan, _)| plan);
    let mut result = VideoPipeline::start_with_crop(
        cx.platform(),
        &target.display,
        target.header_id,
        cx.conn.clone(),
        epoch as u8,
        plan.max_width,
        plan.fps as u16,
        plan.bitrate_kbps,
        &cx.st.stream.codecs,
        cx.info.client_features & features::HQ_REFRESH != 0,
        cx.options.dump.clone(),
        crop_plan,
    );
    if let Err(error) = &result {
        if crop_plan.is_some() {
            tracing::warn!(%error, "viewport crop failed; falling back to full frame");
            super::viewport::crop_failed(cx);
            result = VideoPipeline::start_with_crop(
                cx.platform(),
                &target.display,
                target.header_id,
                cx.conn.clone(),
                epoch as u8,
                plan.max_width,
                plan.fps as u16,
                plan.bitrate_kbps,
                &cx.st.stream.codecs,
                cx.info.client_features & features::HQ_REFRESH != 0,
                cx.options.dump.clone(),
                None,
            );
        }
    }
    match result {
        Ok(pipeline) => {
            cx.st.stream.epoch = epoch;
            let _ = cx
                .st
                .input
                .display_tx
                .send(Some((target.display.clone(), target.header_id)));
            cursor::set_stream_display(cx, target.display.id, target.header_id);
            let (cursor_x, cursor_y, cursor_visible) = cursor::stream_config_cursor(cx);
            cx.send(pb::StreamConfig {
                epoch: cx.st.stream.epoch,
                display_id: target.wire_id,
                codec: pipeline.codec() as i32,
                width: pipeline.width,
                height: pipeline.height,
                fps: u32::from(pipeline.fps),
                bitrate_kbps: plan.bitrate_kbps,
                cursor_x,
                cursor_y,
                cursor_visible,
                crop: pipeline.crop().map(|crop| pb::PixelRect {
                    x: crop.rect.x,
                    y: crop.rect.y,
                    w: crop.rect.w,
                    h: crop.rect.h,
                }),
                viewport_seq: if pipeline.crop().is_some() {
                    crop_seq
                } else {
                    0
                },
                display_width_px: target.display.width_px,
                display_height_px: target.display.height_px,
            });
            cx.st.stream.pipeline = Some(pipeline);
            cx.st.stream.target = Some(target);
            super::on_stream_started(cx);
            true
        }
        Err(error) => {
            tracing::warn!(%error, "capture failed");
            cx.send(pb::ErrorInfo {
                code: u32::from(
                    if matches!(error, PlatformError::PermissionDenied(_)) {
                        ErrorCode::MissingPermissions
                    } else {
                        ErrorCode::Other
                    }
                    .code(),
                ),
                message: error.to_string(),
                ..Default::default()
            });
            false
        }
    }
}
pub fn rebuild<P: Platform>(cx: &mut ConnCx<'_, P>, target: StreamTarget) {
    cx.st.stream.target = Some(target);
    if cx.st.stream.gate.wants_frames() {
        drop(cx.st.stream.pipeline.take());
        start_pipeline(cx);
    }
}
pub fn stop<P: Platform>(cx: &mut ConnCx<'_, P>) {
    drop(cx.st.stream.pipeline.take());
    super::on_stream_stopped(cx);
}
pub fn current_target<P: Platform>(cx: &ConnCx<'_, P>) -> Option<StreamTarget> {
    cx.st.stream.target.clone()
}
pub fn on_pause<P: Platform>(cx: &mut ConnCx<'_, P>) {
    let now = cx.now_us();
    if cx
        .st
        .stream
        .gate
        .on_pause(now)
        .contains(&GateAction::StopEncoder)
    {
        stop(cx);
    }
}
pub fn on_resume<P: Platform>(cx: &mut ConnCx<'_, P>) {
    let now = cx.now_us();
    let actions = cx.st.stream.gate.on_resume(now);
    if actions.contains(&GateAction::StartEncoder) {
        start_pipeline(cx);
    } else if actions.contains(&GateAction::ForceKeyframe) {
        on_keyframe_request(cx, pb::KeyframeRequest::default());
    }
}
pub fn on_keyframe_request<P: Platform>(cx: &mut ConnCx<'_, P>, _m: pb::KeyframeRequest) {
    if let Some(pipeline) = &cx.st.stream.pipeline {
        pipeline.request_keyframe();
    }
}
