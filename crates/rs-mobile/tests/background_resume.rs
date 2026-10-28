mod support;

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use host_core::clock::Clock;
use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_diag::set_setting,
    api_sessions as sessions,
    dto_core::{OpenIntent, OpenOpts, SessionPhase},
    runtime::Core,
    store::PairedHostRow,
    tables::{SessionEntry, SessionTable, VideoConsumer},
};
use rsp_client::{CursorEvent, ReceivedFrame, SessionStateKind};
use rsp_core::{hid::InputMsg, pb, trust::Permission};
use serde_json::Value;
use tokio::time::{sleep, timeout};

const SECOND: Duration = Duration::from_secs(1);
const TWO_SECONDS: Duration = Duration::from_secs(2);

#[derive(Default)]
struct OffsetClock(AtomicU64);

impl OffsetClock {
    fn skip(&self, seconds: u64) {
        self.0.fetch_add(seconds * 1_000_000, Ordering::Relaxed);
    }
}

impl Clock for OffsetClock {
    fn now_micros(&self) -> u64 {
        rsp_core::clock::unix_micros().saturating_add(self.0.load(Ordering::Relaxed))
    }
}

async fn until(limit: Duration, predicate: impl Fn() -> bool) {
    timeout(limit, async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition before deadline");
}

#[derive(Default)]
struct VideoRecorder {
    frames: Mutex<Vec<bool>>,
    closed: AtomicBool,
}

impl VideoRecorder {
    fn keyframes(&self) -> usize {
        self.frames
            .lock()
            .unwrap()
            .iter()
            .filter(|keyframe| **keyframe)
            .count()
    }
}

impl VideoConsumer for VideoRecorder {
    fn frame(&self, frame: &ReceivedFrame) {
        self.frames.lock().unwrap().push(frame.frame.keyframe);
    }
    fn stream_config(&self, _: &pb::StreamConfig) {}
    fn cursor(&self, _: &CursorEvent) {}
    fn cursor_msg(&self, _: &pb::CursorMsg) {}
    fn clock(&self, _: u64, _: i64) {}
    fn closed(&self, _: &str) {
        self.closed.store(true, Ordering::Release);
    }
}

struct Fixture {
    core: Arc<Core>,
    events: Arc<support::EventCollector>,
    core_clock: Arc<OffsetClock>,
    host_clock: Arc<OffsetClock>,
    _host: TestHost,
    platform: FakePlatform,
    entry: Arc<SessionEntry>,
    video: Arc<VideoRecorder>,
}

impl Fixture {
    async fn new(hold: bool) -> Self {
        let core_clock = Arc::new(OffsetClock::default());
        let core = support::test_core_with(|_, options| {
            let clock = core_clock.clone();
            options.clock = Some(Arc::new(move || clock.now_micros()));
            options.worker_threads = 2;
        });
        support::with_identity(&core, "Background resume test device");
        let events = support::EventCollector::new();
        core.events().subscribe(events.clone());
        let platform = FakePlatform::new();
        let host_clock = Arc::new(OffsetClock::default());
        let host = TestHost::start_with_platform(
            platform.clone(),
            TestHostOptions {
                tcp: false,
                services_clock: Some(host_clock.clone()),
                ..Default::default()
            },
        )
        .await;
        host.pair(&core.local_identity().unwrap(), Permission::Control);
        core.store()
            .upsert_paired_host(&PairedHostRow {
                host_id: host.host_id.clone(),
                fp: host.fp,
                name: "Background resume test host".into(),
                addresses: vec![host.quic_addr],
                paired_at_us: core.now_us(),
                permission: Some("control".into()),
            })
            .unwrap();
        if hold {
            set_setting(&core, "device.backgroundHold", "true").unwrap();
        }
        let id = sessions::open(
            &core,
            &host.host_id,
            OpenOpts {
                intent: OpenIntent::Control,
                display_id: None,
                quality: None,
                mode: None,
                audio: None,
                max_fps: None,
                max_width: None,
                codec: None,
            },
        )
        .unwrap();
        let entry = core.state::<SessionTable>().get(id).unwrap();
        until(TWO_SECONDS, || {
            entry.summary().phase == SessionPhase::Streaming && entry.summary().display_id.is_some()
        })
        .await;
        let video = Arc::new(VideoRecorder::default());
        assert!(entry.video.attach(video.clone()));
        until(SECOND, || video.keyframes() > 0).await;
        assert!(core.events().flush(SECOND));
        Self {
            core,
            events,
            core_clock,
            host_clock,
            _host: host,
            platform,
            entry,
            video,
        }
    }

    fn lifecycle(&self) -> Vec<Value> {
        self.events
            .events()
            .into_iter()
            .map(|(_, value)| value)
            .filter(|value| value["type"] == "session.state" || value["type"] == "session.error")
            .collect()
    }

    async fn background(&self) {
        self.platform.clear_injected();
        sessions::background(&self.core, self.entry.id, true).unwrap();
        // Observe the input effect before jumping the injected clock: the
        // background notification is consumed by a separate Core worker.
        until(SECOND, || {
            self.platform
                .injected_msgs()
                .contains(&InputMsg::ReleaseAll)
        })
        .await;
    }

    async fn goodbye(&self) -> usize {
        let before = self.lifecycle().len();
        self.background().await;
        self.core_clock.skip(31);
        until(TWO_SECONDS, || self.entry.handle().is_none()).await;
        assert!(self.core.events().flush(SECOND));
        assert_eq!(
            self.lifecycle().len(),
            before,
            "background close must be invisible to JS"
        );
        assert!(!self.video.closed.load(Ordering::Acquire));
        before
    }

    async fn foreground(&self, before: usize, resumed: bool) {
        let keyframes = self.video.keyframes();
        sessions::background(&self.core, self.entry.id, false).unwrap();
        until(TWO_SECONDS, || {
            self.lifecycle()
                .get(before + 1)
                .is_some_and(|event| event["phase"] == "streaming")
        })
        .await;
        let events = self.lifecycle();
        let reconnect = &events[before];
        assert_eq!(reconnect["phase"], "reconnecting");
        assert_eq!(reconnect["reconnectReason"], "resume");
        assert_eq!(events[before + 1]["phase"], "streaming");
        assert_eq!(events[before + 1]["resumed"], resumed);
        assert!(events[before + 1].get("reconnectReason").is_none());
        assert_eq!(events[before + 1]["sessionId"], self.entry.id.to_string());
        until(SECOND, || self.video.keyframes() > keyframes).await;
        let all: Vec<_> = self
            .events
            .events()
            .into_iter()
            .map(|(_, value)| value)
            .collect();
        let reconnect_at = all
            .iter()
            .rposition(|value| value["phase"] == "reconnecting")
            .unwrap();
        let resumed = &all[reconnect_at + 1..];
        let role = resumed
            .iter()
            .position(|value| value["type"] == "session.role")
            .unwrap();
        let displays = resumed
            .iter()
            .position(|value| value["type"] == "session.displays")
            .unwrap();
        let streaming = resumed
            .iter()
            .position(|value| value["phase"] == "streaming")
            .unwrap();
        assert!(role < displays && displays < streaming);
    }

    async fn input_and_same_id(&self) {
        self.platform.clear_injected();
        sessions::key(&self.core, self.entry.id, 0x04, true, 0).unwrap();
        until(SECOND, || {
            self.platform.injected_msgs().iter().any(|msg| {
                matches!(
                    msg,
                    InputMsg::Key {
                        usage: 0x04,
                        down: true,
                        ..
                    }
                )
            })
        })
        .await;
        let sessions = sessions::list(&self.core);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, self.entry.id.to_string());
    }

    async fn ended(&self, before: usize, reason: &str) {
        until(TWO_SECONDS, || {
            self.lifecycle().len() >= before + 2 && self.core.state::<SessionTable>().is_empty()
        })
        .await;
        let events = self.lifecycle();
        assert_eq!(events[before]["type"], "session.error");
        assert_eq!(events[before]["reason"], reason);
        assert_eq!(events[before + 1]["phase"], "ended");
        until(SECOND, || self.video.closed.load(Ordering::Acquire)).await;
    }

    async fn close(&self) {
        sessions::close(&self.core, self.entry.id);
        until(TWO_SECONDS, || self.core.state::<SessionTable>().is_empty()).await;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        sessions::close(&self.core, self.entry.id);
        self.core.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_resume_within_window_keeps_session_id_and_accepts_input() {
    let fixture = Fixture::new(false).await;
    let original = fixture.entry.handle().unwrap();
    let before = fixture.goodbye().await;
    assert_eq!(original.state(), SessionStateKind::Ended);
    fixture.core_clock.skip(19);
    fixture.foreground(before, true).await;
    fixture.input_and_same_id().await;
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_resume_expiry_ends_session_and_closes_video() {
    let fixture = Fixture::new(false).await;
    let before = fixture.goodbye().await;
    fixture.core_clock.skip(60);
    sessions::background(&fixture.core, fixture.entry.id, false).unwrap();
    fixture.ended(before, "background").await;
    assert!(fixture.core.state::<SessionTable>().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_resume_host_expiry_falls_back_to_a_new_host_session() {
    let fixture = Fixture::new(false).await;
    let before = fixture.goodbye().await;
    fixture.host_clock.skip(61);
    sleep(Duration::from_millis(1500)).await;
    fixture.core_clock.skip(10);
    fixture.foreground(before, false).await;
    fixture.input_and_same_id().await;
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_resume_short_pause_uses_the_same_connection() {
    let fixture = Fixture::new(false).await;
    let original = fixture.entry.handle().unwrap();
    let token = original.resume_token();
    let before = fixture.lifecycle().len();
    fixture.background().await;
    fixture.core_clock.skip(10);
    let keyframes = fixture.video.keyframes();
    sessions::background(&fixture.core, fixture.entry.id, false).unwrap();
    until(SECOND, || fixture.video.keyframes() > keyframes).await;
    assert_eq!(original.state(), SessionStateKind::Streaming);
    assert_eq!(fixture.entry.handle().unwrap().resume_token(), token);
    assert!(fixture.core.events().flush(SECOND));
    assert_eq!(fixture.lifecycle().len(), before);
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_resume_hold_keeps_the_connection_alive() {
    let fixture = Fixture::new(true).await;
    let original = fixture.entry.handle().unwrap();
    let token = original.resume_token();
    let before = fixture.lifecycle().len();
    fixture.background().await;
    fixture.core_clock.skip(31);
    sleep(TWO_SECONDS).await;
    assert!(fixture.entry.handle().is_some());
    assert_eq!(original.state(), SessionStateKind::Streaming);
    assert_eq!(fixture.entry.handle().unwrap().resume_token(), token);
    assert_eq!(fixture.lifecycle().len(), before);
    let keyframes = fixture.video.keyframes();
    sessions::background(&fixture.core, fixture.entry.id, false).unwrap();
    until(SECOND, || fixture.video.keyframes() > keyframes).await;
    fixture.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_resume_close_while_waiting_ends_with_user_closed() {
    let fixture = Fixture::new(false).await;
    let before = fixture.goodbye().await;
    sessions::close(&fixture.core, fixture.entry.id);
    fixture.ended(before, "userClosed").await;
    assert!(fixture.core.state::<SessionTable>().is_empty());
}
