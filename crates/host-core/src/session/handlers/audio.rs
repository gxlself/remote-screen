//! Optional system audio shared by active screen sessions.
use super::ConnCx;
use crate::{
    auth,
    pipeline::audio::{video_encoder_kbps, AudioHub, AudioSender},
    platform::Platform,
    session::manager::{EndReason, HostServices, SessionManager},
};
use rs_audio::codec::EncoderConfig;
use rsp_core::{features, pb, ratectl::Decision};
use std::sync::Arc;

pub struct Host {
    pub hub: AudioHub,
}
impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self {
            hub: AudioHub::new(EncoderConfig::default()),
        }
    }
}

#[derive(Default)]
pub struct Conn {
    pub caps_audio: bool,
    pub requested: Option<bool>,
    pub sender: Option<AudioSender>,
    pub next_seq: u16,
    pub warned: bool,
}

#[derive(Debug)]
pub enum Event {}
pub const FEATURES: u64 = features::AUDIO_OPUS;

fn requested<P: Platform>(cx: &ConnCx<'_, P>) -> bool {
    cx.info.intent != pb::Intent::Files
        && cx.info.client_features & FEATURES != 0
        && cx.st.audio.requested.unwrap_or(cx.st.audio.caps_audio)
}

fn wanted_ignoring_stream<P: Platform>(cx: &ConnCx<'_, P>) -> bool {
    requested(cx) && cx.platform().audio_capture_supported()
}

fn wanted<P: Platform>(cx: &ConnCx<'_, P>) -> bool {
    cx.st.stream.pipeline.is_some() && wanted_ignoring_stream(cx)
}

pub fn video_kbps<P: Platform>(cx: &ConnCx<'_, P>, d: &Decision) -> u32 {
    video_encoder_kbps(d, wanted_ignoring_stream(cx))
}

fn stop<P: Platform>(cx: &mut ConnCx<'_, P>) {
    if let Some(sender) = cx.st.audio.sender.take() {
        cx.st.audio.next_seq = sender.next_seq();
        drop(sender);
    }
}

fn update<P: Platform>(cx: &mut ConnCx<'_, P>) {
    if !wanted(cx) {
        stop(cx);
        if requested(cx)
            && cx.st.stream.pipeline.is_some()
            && !cx.platform().audio_capture_supported()
            && !cx.st.audio.warned
        {
            tracing::warn!("system audio capture is unsupported");
            cx.st.audio.warned = true;
        }
        return;
    }
    if cx.st.audio.sender.is_some() {
        return;
    }
    match cx.mgr.features().audio.hub.subscribe(cx.platform()) {
        Ok(sub) => {
            cx.st.audio.sender = Some(AudioSender::spawn(
                cx.conn.clone(),
                sub,
                cx.st.audio.next_seq,
            ));
        }
        Err(error) => {
            if !cx.st.audio.warned {
                tracing::warn!(%error, "system audio capture failed");
                cx.st.audio.warned = true;
            }
        }
    }
}

pub fn on_set_audio<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::SetAudio) {
    if cx.info.client_features & FEATURES == 0 {
        tracing::debug!("ignoring SetAudio without AUDIO_OPUS");
        return;
    }
    cx.st.audio.requested = Some(m.enabled);
    update(cx);
}

pub fn on_stream_started<P: Platform>(cx: &mut ConnCx<'_, P>) {
    update(cx);
}
pub fn on_stream_stopped<P: Platform>(cx: &mut ConnCx<'_, P>) {
    stop(cx);
}
pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    stop(cx);
}
pub fn on_host_start<P: Platform>(_mgr: &Arc<SessionManager<P>>) {}
pub fn on_admitted<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}
pub fn on_event<P: Platform>(_cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {}
}
pub fn on_tick<P: Platform>(_cx: &mut ConnCx<'_, P>, _now_us: u64) {}
pub fn deadline<P: Platform>(_cx: &ConnCx<'_, P>) -> Option<u64> {
    None
}
