//! Fixed feature registry and hook order for the host runtime.
use crate::{
    auth,
    platform::Platform,
    session::manager::{ConnInfo, ConnKey, EndReason, HostServices, SessionManager},
    RuntimeOptions,
};
use rsp_core::{features, pb};
use std::{future::Future, sync::Arc, time::Duration};
pub mod activity;
pub mod approval;
pub mod audio;
pub mod clipboard;
pub mod cursor;
pub mod displays;
pub mod files;
pub mod files_done;
pub mod idle;
pub mod input;
pub mod power;
pub mod rate;
pub mod recording;
pub mod roster;
pub mod stream;
pub mod takeover;
pub mod vdisplay;
pub mod viewport;
pub struct HostFeatures {
    pub roster: roster::Host,
    pub approval: approval::Host,
    pub takeover: takeover::Host,
    pub activity: activity::Host,
    pub idle: idle::Host,
    pub displays: displays::Host,
    pub cursor: cursor::Host,
    pub clipboard: clipboard::Host,
    pub power: power::Host,
    pub stream: stream::Host,
    pub rate: rate::Host,
    pub files: files::Host,
    pub input: input::Host,
    pub audio: audio::Host,
    pub recording: recording::Host,
    pub vdisplay: vdisplay::Host,
    pub viewport: viewport::Host,
}
impl HostFeatures {
    pub fn new(s: &HostServices) -> Self {
        Self {
            roster: roster::Host::new(s),
            approval: approval::Host::new(s),
            takeover: takeover::Host::new(s),
            activity: activity::Host::new(s),
            idle: idle::Host::new(s),
            displays: displays::Host::new(s),
            cursor: cursor::Host::new(s),
            clipboard: clipboard::Host::new(s),
            power: power::Host::new(s),
            stream: stream::Host::new(s),
            rate: rate::Host::new(s),
            files: files::Host::new(s),
            input: input::Host::new(s),
            audio: audio::Host::new(s),
            recording: recording::Host::new(s),
            vdisplay: vdisplay::Host::new(s),
            viewport: viewport::Host::new(s),
        }
    }
}
#[derive(Default)]
pub struct ConnFeatures {
    pub roster: roster::Conn,
    pub approval: approval::Conn,
    pub takeover: takeover::Conn,
    pub activity: activity::Conn,
    pub idle: idle::Conn,
    pub displays: displays::Conn,
    pub cursor: cursor::Conn,
    pub clipboard: clipboard::Conn,
    pub power: power::Conn,
    pub stream: stream::Conn,
    pub rate: rate::Conn,
    pub files: files::Conn,
    pub input: input::Conn,
    pub audio: audio::Conn,
    pub recording: recording::Conn,
    pub vdisplay: vdisplay::Conn,
    pub viewport: viewport::Conn,
}
#[derive(Debug)]
pub enum FeatureEvent {
    Roster(roster::Event),
    Approval(approval::Event),
    Takeover(takeover::Event),
    Activity(activity::Event),
    Idle(idle::Event),
    Displays(displays::Event),
    Cursor(cursor::Event),
    Clipboard(clipboard::Event),
    Power(power::Event),
    Stream(stream::Event),
    Rate(rate::Event),
    Files(files::Event),
    Input(input::Event),
    Audio(audio::Event),
    Recording(recording::Event),
    Vdisplay(vdisplay::Event),
    Viewport(viewport::Event),
}
pub fn host_features() -> u64 {
    features::TCP_TRANSPORT
        | features::FEC_RS
        | features::MULTI_DISPLAY
        | roster::FEATURES
        | approval::FEATURES
        | takeover::FEATURES
        | activity::FEATURES
        | idle::FEATURES
        | displays::FEATURES
        | cursor::FEATURES
        | clipboard::FEATURES
        | power::FEATURES
        | stream::FEATURES
        | rate::FEATURES
        | files::FEATURES
        | input::FEATURES
        | audio::FEATURES
        | recording::FEATURES
        | vdisplay::FEATURES
        | viewport::FEATURES
        | features::PAIR_FILE
        | features::PAIR_PASSWORD
}
pub struct ConnCx<'a, P: Platform> {
    pub mgr: &'a Arc<SessionManager<P>>,
    pub key: ConnKey,
    pub conn: &'a rsp_transport::Conn,
    pub info: &'a mut ConnInfo,
    pub options: &'a RuntimeOptions,
    pub st: &'a mut ConnFeatures,
    pub out: &'a mut Vec<pb::Envelope>,
    pub end: &'a mut Option<EndReason>,
    pub tasks: &'a mut tokio::task::JoinSet<()>,
}
impl<P: Platform> ConnCx<'_, P> {
    pub fn send(&mut self, env: impl Into<pb::Envelope>) {
        self.out.push(env.into());
    }
    pub fn end(&mut self, reason: EndReason) {
        *self.end = Some(reason);
    }
    pub fn now_us(&self) -> u64 {
        self.mgr.now_us()
    }
    pub fn platform(&self) -> &P {
        self.mgr.platform().as_ref()
    }
    pub fn is_controller(&self) -> bool {
        self.info.role == auth::Role::Controller
    }
    pub fn spawn(&mut self, f: impl Future<Output = ()> + Send + 'static) {
        self.tasks.spawn(f);
    }
    pub fn sync_info(&self) {
        self.mgr.update(self.key, |info| *info = self.info.clone());
    }
}
pub fn on_host_start<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    roster::on_host_start(mgr);
    approval::on_host_start(mgr);
    activity::on_host_start(mgr);
    displays::on_host_start(mgr);
    cursor::on_host_start(mgr);
    clipboard::on_host_start(mgr);
    power::on_host_start(mgr);
    takeover::on_host_start(mgr);
    idle::on_host_start(mgr);
    stream::on_host_start(mgr);
    rate::on_host_start(mgr);
    files::on_host_start(mgr);
    input::on_host_start(mgr);
    audio::on_host_start(mgr);
    recording::on_host_start(mgr);
    vdisplay::on_host_start(mgr);
    viewport::on_host_start(mgr);
}
pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    roster::on_admitted(cx);
    activity::on_admitted(cx);
    displays::on_admitted(cx);
    cursor::on_admitted(cx);
    clipboard::on_admitted(cx);
    files::on_admitted(cx);
    input::on_admitted(cx);
    idle::on_admitted(cx);
    power::on_admitted(cx);
    takeover::on_admitted(cx);
    approval::on_admitted(cx);
    stream::on_admitted(cx);
    rate::on_admitted(cx);
    audio::on_admitted(cx);
    recording::on_admitted(cx);
    vdisplay::on_admitted(cx);
    viewport::on_admitted(cx);
}
pub fn recompute_gate<P: Platform>(cx: &mut ConnCx<'_, P>) {
    let paused = activity::paused_watch(cx.mgr).is_some_and(|watch| *watch.borrow());
    let prompt_open = approval::prompt_watch(cx.mgr).is_some_and(|watch| *watch.borrow());
    let admitted = cx.mgr.info(cx.key).is_some();
    let open = admitted && cx.is_controller() && !paused && !prompt_open;
    if open == cx.st.input.gate_open {
        return;
    }
    cx.st.input.gate_open = open;
    let _ = cx.st.input.gate_tx.send(open);
    if !open {
        cx.platform().release_all();
    }
    input::on_gate_change(cx, open);
}

pub fn on_uni_stream<P: Platform>(
    cx: &mut ConnCx<'_, P>,
    kind: u8,
    mut recv: rsp_transport::UniRecv,
) {
    match kind {
        0x03 => clipboard::on_stream(cx, kind, recv),
        0x04 => files::on_stream(cx, kind, recv),
        _ => {
            let _ = recv.stop(1);
        }
    }
}

/// Recheck the host clock at least every second, including offset clock jumps.
pub(crate) fn timer_delay(deadline: Option<u64>, now_us: u64) -> Duration {
    Duration::from_micros(deadline.map_or(1_000_000, |at| at.saturating_sub(now_us).min(1_000_000)))
}

pub fn on_role_changed<P: Platform>(cx: &mut ConnCx<'_, P>, old: auth::Role) {
    roster::on_role_changed(cx, old);
    input::on_role_changed(cx, old);
    cursor::on_role_changed(cx, old);
    clipboard::on_role_changed(cx, old);
    idle::on_role_changed(cx, old);
    takeover::on_role_changed(cx, old);
    approval::on_role_changed(cx, old);
    activity::on_role_changed(cx, old);
    displays::on_role_changed(cx, old);
    power::on_role_changed(cx, old);
    stream::on_role_changed(cx, old);
    rate::on_role_changed(cx, old);
    files::on_role_changed(cx, old);
    audio::on_role_changed(cx, old);
    recording::on_role_changed(cx, old);
    vdisplay::on_role_changed(cx, old);
    viewport::on_role_changed(cx, old);
}
pub fn on_stream_started<P: Platform>(cx: &mut ConnCx<'_, P>) {
    power::on_stream_started(cx);
    cursor::on_stream_started(cx);
    rate::on_stream_started(cx);
    idle::on_stream_started(cx);
    roster::on_stream_started(cx);
    approval::on_stream_started(cx);
    takeover::on_stream_started(cx);
    activity::on_stream_started(cx);
    displays::on_stream_started(cx);
    clipboard::on_stream_started(cx);
    stream::on_stream_started(cx);
    files::on_stream_started(cx);
    input::on_stream_started(cx);
    audio::on_stream_started(cx);
    recording::on_stream_started(cx);
    vdisplay::on_stream_started(cx);
    viewport::on_stream_started(cx);
}
pub fn on_stream_stopped<P: Platform>(cx: &mut ConnCx<'_, P>) {
    power::on_stream_stopped(cx);
    cursor::on_stream_stopped(cx);
    rate::on_stream_stopped(cx);
    roster::on_stream_stopped(cx);
    approval::on_stream_stopped(cx);
    takeover::on_stream_stopped(cx);
    activity::on_stream_stopped(cx);
    idle::on_stream_stopped(cx);
    displays::on_stream_stopped(cx);
    clipboard::on_stream_stopped(cx);
    stream::on_stream_stopped(cx);
    files::on_stream_stopped(cx);
    input::on_stream_stopped(cx);
    audio::on_stream_stopped(cx);
    recording::on_stream_stopped(cx);
    vdisplay::on_stream_stopped(cx);
    viewport::on_stream_stopped(cx);
}
pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, reason: EndReason) {
    input::on_closed(cx, reason);
    takeover::on_closed(cx, reason);
    idle::on_closed(cx, reason);
    cursor::on_closed(cx, reason);
    clipboard::on_closed(cx, reason);
    files::on_closed(cx, reason);
    stream::on_closed(cx, reason);
    rate::on_closed(cx, reason);
    displays::on_closed(cx, reason);
    activity::on_closed(cx, reason);
    power::on_closed(cx, reason);
    approval::on_closed(cx, reason);
    roster::on_closed(cx, reason);
    audio::on_closed(cx, reason);
    recording::on_closed(cx, reason);
    vdisplay::on_closed(cx, reason);
    viewport::on_closed(cx, reason);
}
pub fn on_tick<P: Platform>(cx: &mut ConnCx<'_, P>, now_us: u64) {
    roster::on_tick(cx, now_us);
    approval::on_tick(cx, now_us);
    takeover::on_tick(cx, now_us);
    activity::on_tick(cx, now_us);
    idle::on_tick(cx, now_us);
    displays::on_tick(cx, now_us);
    cursor::on_tick(cx, now_us);
    clipboard::on_tick(cx, now_us);
    power::on_tick(cx, now_us);
    stream::on_tick(cx, now_us);
    rate::on_tick(cx, now_us);
    files::on_tick(cx, now_us);
    input::on_tick(cx, now_us);
    audio::on_tick(cx, now_us);
    recording::on_tick(cx, now_us);
    vdisplay::on_tick(cx, now_us);
    viewport::on_tick(cx, now_us);
}
pub fn deadline<P: Platform>(cx: &ConnCx<'_, P>) -> Option<u64> {
    [
        roster::deadline(cx),
        approval::deadline(cx),
        takeover::deadline(cx),
        activity::deadline(cx),
        idle::deadline(cx),
        displays::deadline(cx),
        cursor::deadline(cx),
        clipboard::deadline(cx),
        power::deadline(cx),
        stream::deadline(cx),
        rate::deadline(cx),
        files::deadline(cx),
        input::deadline(cx),
        audio::deadline(cx),
        recording::deadline(cx),
        vdisplay::deadline(cx),
        viewport::deadline(cx),
    ]
    .into_iter()
    .flatten()
    .min()
}
pub fn on_event<P: Platform>(cx: &mut ConnCx<'_, P>, ev: FeatureEvent) {
    match ev {
        FeatureEvent::Roster(ev) => roster::on_event(cx, ev),
        FeatureEvent::Approval(ev) => approval::on_event(cx, ev),
        FeatureEvent::Takeover(ev) => takeover::on_event(cx, ev),
        FeatureEvent::Activity(ev) => activity::on_event(cx, ev),
        FeatureEvent::Idle(ev) => idle::on_event(cx, ev),
        FeatureEvent::Displays(ev) => displays::on_event(cx, ev),
        FeatureEvent::Cursor(ev) => cursor::on_event(cx, ev),
        FeatureEvent::Clipboard(ev) => clipboard::on_event(cx, ev),
        FeatureEvent::Power(ev) => power::on_event(cx, ev),
        FeatureEvent::Stream(ev) => stream::on_event(cx, ev),
        FeatureEvent::Rate(ev) => rate::on_event(cx, ev),
        FeatureEvent::Files(ev) => files::on_event(cx, ev),
        FeatureEvent::Input(ev) => input::on_event(cx, ev),
        FeatureEvent::Audio(ev) => audio::on_event(cx, ev),
        FeatureEvent::Recording(ev) => recording::on_event(cx, ev),
        FeatureEvent::Vdisplay(ev) => vdisplay::on_event(cx, ev),
        FeatureEvent::Viewport(ev) => viewport::on_event(cx, ev),
    }
}
pub fn on_control<P: Platform>(cx: &mut ConnCx<'_, P>, msg: pb::envelope::Msg) -> bool {
    use pb::envelope::Msg;
    match msg {
        Msg::StartStream(m) => stream::on_start_stream(cx, m),
        Msg::PauseStream(_) => stream::on_pause(cx),
        Msg::ResumeStream(_) => stream::on_resume(cx),
        Msg::KeyframeRequest(m) => stream::on_keyframe_request(cx, m),
        Msg::SelectDisplay(m) => displays::on_select_display(cx, m),
        Msg::RequestControl(_) => takeover::on_request_control(cx),
        Msg::ControlResponse(m) => takeover::on_control_response(cx, m),
        Msg::SessionAction(m) => idle::on_session_action(cx, m),
        Msg::Feedback(m) => rate::on_feedback(cx, m),
        Msg::SetQuality(m) => rate::on_set_quality(cx, m),
        Msg::FileList(m) => files::on_file_list(cx, m),
        Msg::FileRequest(m) => files::on_file_request(cx, m),
        Msg::FileQuery(m) => files::on_file_query(cx, m),
        Msg::FileStatus(m) => files::on_file_status(cx, m),
        Msg::FileCancel(m) => files::on_file_cancel(cx, m),
        Msg::SetAudio(m) => audio::on_set_audio(cx, m),
        Msg::RecordingNotice(m) => recording::on_notice(cx, m),
        Msg::CreateVirtualDisplay(m) => vdisplay::on_create(cx, m),
        Msg::ReleaseVirtualDisplay(m) => vdisplay::on_release(cx, m),
        Msg::Viewport(m) => viewport::on_viewport(cx, m),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::timer_delay;
    use std::time::Duration;

    #[test]
    fn timer_delay_clamps_deadlines_to_one_second() {
        let now = 10_000_000;
        assert_eq!(timer_delay(None, now), Duration::from_secs(1));
        assert_eq!(timer_delay(Some(now - 1), now), Duration::ZERO);
        assert_eq!(
            timer_delay(Some(now + 5_000_000), now),
            Duration::from_secs(1)
        );
        assert_eq!(
            timer_delay(Some(now + 300_000), now),
            Duration::from_millis(300)
        );
    }
}
