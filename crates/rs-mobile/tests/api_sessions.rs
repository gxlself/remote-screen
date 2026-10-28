mod support;

use std::{
    net::{SocketAddr, UdpSocket},
    sync::{Arc, Mutex},
    time::Duration,
};

use host_core::platform::{ClipboardItem as PlatformClipboardItem, Platform};
use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_sessions as sessions,
    dto_core::{OpenIntent, OpenOpts, Quality, Role, SessionPhase},
    error::CoreError,
    runtime::Core,
    settings::SettingsState,
    store::PairedHostRow,
    tables::{SessionEntry, SessionTable, VideoConsumer},
};
use rsp_client::{CursorEvent, PrewarmPool, ReceivedFrame};
use rsp_core::{
    hid::{InputMsg, Mods, MAX_TEXT_BYTES},
    pb,
    trust::Permission,
};
use tokio::time::{sleep, timeout, Instant};

pub const SECOND: Duration = Duration::from_secs(1);
pub const TWO_SECONDS: Duration = Duration::from_secs(2);

pub fn opts(intent: OpenIntent) -> OpenOpts {
    OpenOpts {
        intent,
        display_id: None,
        quality: None,
        mode: None,
        audio: None,
        max_fps: None,
        max_width: None,
        codec: None,
    }
}

pub async fn until(limit: Duration, predicate: impl Fn() -> bool) {
    timeout(limit, async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition before deadline");
}

pub fn new_core() -> (Arc<Core>, Arc<support::EventCollector>) {
    let core = support::test_core_with(|_, options| options.worker_threads = 2);
    support::with_identity(&core, "Session test device");
    let events = support::EventCollector::new();
    core.events().subscribe(events.clone());
    (core, events)
}

pub fn pair_row(core: &Arc<Core>, host_id: &str, fp: [u8; 32], address: SocketAddr) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host_id.into(),
            fp,
            name: "Paired test host".into(),
            addresses: vec![address],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();
}

pub struct Fixture {
    pub core: Arc<Core>,
    pub events: Arc<support::EventCollector>,
    pub host: TestHost,
    pub platform: FakePlatform,
}

impl Fixture {
    pub async fn new() -> Self {
        let (core, events) = new_core();
        let mut displays = FakePlatform::new().displays();
        let mut second = displays[0].clone();
        second.id = 2;
        second.primary = false;
        displays.push(second);
        let platform = FakePlatform::with_displays(displays);
        let host = TestHost::start_with_platform(
            platform.clone(),
            TestHostOptions {
                // These sessions dial QUIC. An unrelated TCP bind on the same
                // ephemeral number can fail when fixtures start concurrently.
                tcp: false,
                ..Default::default()
            },
        )
        .await;
        host.pair(&core.local_identity().unwrap(), Permission::Control);
        pair_row(&core, &host.host_id, host.fp, host.quic_addr);
        Self {
            core,
            events,
            host,
            platform,
        }
    }

    pub async fn streaming(&self, intent: OpenIntent) -> Arc<SessionEntry> {
        let id = sessions::open(&self.core, &self.host.host_id, opts(intent)).unwrap();
        let entry = self.core.state::<SessionTable>().get(id).unwrap();
        until(TWO_SECONDS, || {
            entry.summary().phase == SessionPhase::Streaming && entry.summary().display_id.is_some()
        })
        .await;
        entry
    }

    pub async fn close(&self, id: u64) {
        sessions::close(&self.core, id);
        until(TWO_SECONDS, || self.core.state::<SessionTable>().is_empty()).await;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for entry in self.core.state::<SessionTable>().list() {
            sessions::close(&self.core, entry.id);
        }
        self.core.shutdown();
    }
}

#[derive(Debug, Clone)]
pub enum VideoEvent {
    Config(u32),
    Frame { keyframe: bool },
    Closed(String),
}

#[derive(Default)]
pub struct VideoRecorder {
    pub events: Mutex<Vec<VideoEvent>>,
    pub clocks: Mutex<Vec<(u64, i64)>>,
    pub cursors: Mutex<Vec<CursorEvent>>,
    pub cursor_messages: Mutex<Vec<pb::CursorMsg>>,
}

impl VideoRecorder {
    pub fn events(&self) -> Vec<VideoEvent> {
        self.events.lock().unwrap().clone()
    }
    pub fn frame_count(&self) -> usize {
        self.events()
            .iter()
            .filter(|event| matches!(event, VideoEvent::Frame { .. }))
            .count()
    }
    pub fn clear(&self) {
        self.events.lock().unwrap().clear();
    }
}

impl VideoConsumer for VideoRecorder {
    fn frame(&self, frame: &ReceivedFrame) {
        self.events.lock().unwrap().push(VideoEvent::Frame {
            keyframe: frame.frame.keyframe,
        });
    }
    fn stream_config(&self, config: &pb::StreamConfig) {
        self.events
            .lock()
            .unwrap()
            .push(VideoEvent::Config(config.display_id));
    }
    fn cursor(&self, cursor: &CursorEvent) {
        self.cursors.lock().unwrap().push(cursor.clone());
    }
    fn cursor_msg(&self, message: &pb::CursorMsg) {
        self.cursor_messages.lock().unwrap().push(message.clone());
    }
    fn clock(&self, rtt_us: u64, offset_us: i64) {
        self.clocks.lock().unwrap().push((rtt_us, offset_us));
    }
    fn closed(&self, reason: &str) {
        self.events
            .lock()
            .unwrap()
            .push(VideoEvent::Closed(reason.into()));
    }
}

pub fn terminal(events: &support::EventCollector, id: u64) -> bool {
    let session_id = id.to_string();
    events
        .of_type("session.state")
        .iter()
        .any(|event| event["sessionId"] == session_id.as_str() && event["phase"] == "ended")
}

pub fn user_closed(events: &support::EventCollector, id: u64) -> bool {
    let session_id = id.to_string();
    events
        .of_type("session.error")
        .iter()
        .any(|event| event["sessionId"] == session_id.as_str() && event["reason"] == "userClosed")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn open_returns_id_and_publishes_summary_and_stats() {
    let fixture = Fixture::new().await;
    let started = Instant::now();
    let id = sessions::open(&fixture.core, &fixture.host.host_id, opts(OpenIntent::View)).unwrap();
    assert_eq!(id, 1);
    assert!(started.elapsed() < Duration::from_millis(100));
    until(SECOND, || !fixture.events.events().is_empty()).await;
    let first = &fixture.events.events()[0].1;
    assert_eq!(first["type"], "session.state");
    assert_eq!(first["phase"], "connecting");
    assert_eq!(first["sessionId"], "1");
    assert_eq!(first["hostId"], fixture.host.host_id);
    until(TWO_SECONDS, || {
        let summaries = sessions::list(&fixture.core);
        summaries.len() == 1
            && summaries[0].phase == SessionPhase::Streaming
            && summaries[0].role == Some(Role::Viewer)
            && summaries[0].display_id.is_some()
    })
    .await;
    assert!(fixture.core.events().flush(SECOND));
    let events = fixture.events.events();
    let authenticating = events
        .iter()
        .position(|(_, event)| event["phase"] == "authenticating")
        .unwrap();
    let role = events
        .iter()
        .position(|(_, event)| event["type"] == "session.role")
        .unwrap();
    let displays = events
        .iter()
        .position(|(_, event)| event["type"] == "session.displays")
        .unwrap();
    let streaming = events
        .iter()
        .position(|(_, event)| event["phase"] == "streaming")
        .unwrap();
    assert!(authenticating < role && role < displays && displays < streaming);
    let phases: Vec<_> = events
        .iter()
        .filter(|(_, event)| event["type"] == "session.state")
        .map(|(_, event)| event["phase"].as_str().unwrap())
        .collect();
    assert_eq!(phases, ["connecting", "authenticating", "streaming"]);
    until(TWO_SECONDS, || {
        !fixture.events.of_type("session.stats").is_empty()
    })
    .await;
    fixture.close(id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn video_attach_replays_config_requests_keyframe_and_routes_native_events() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(OpenIntent::View).await;
    let recorder = Arc::new(VideoRecorder::default());
    let requests = entry.handle().unwrap().stats().keyframe_requests;
    assert!(entry.video.attach(recorder.clone()));
    until(SECOND, || {
        matches!(recorder.events().first(), Some(VideoEvent::Config(_)))
    })
    .await;
    until(SECOND, || {
        entry.handle().unwrap().stats().keyframe_requests == requests + 1
    })
    .await;
    until(TWO_SECONDS, || recorder.frame_count() >= 10).await;
    assert!(recorder
        .events()
        .iter()
        .any(|event| matches!(event, VideoEvent::Frame { keyframe: true })));
    assert!(matches!(recorder.events()[0], VideoEvent::Config(1)));
    until(SECOND, || !recorder.clocks.lock().unwrap().is_empty()).await;
    assert!(!recorder.cursors.lock().unwrap().is_empty());
    // Native video, cursor and clock payloads never cross the JS event boundary.
    assert!(fixture.events.events().iter().all(|(_, event)| ![
        "session.frame",
        "session.cursor",
        "session.clock"
    ]
    .contains(&event["type"].as_str().unwrap())));
    fixture.close(entry.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn keyboard_combo_and_utf8_text_reach_host_in_order() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(OpenIntent::Control).await;
    sessions::key(&fixture.core, entry.id, 0x04, true, 0).unwrap();
    sessions::key(&fixture.core, entry.id, 0x04, false, 0).unwrap();
    sessions::combo(&fixture.core, entry.id, "cmdTab").unwrap();
    sessions::text(&fixture.core, entry.id, "你好").unwrap();
    until(TWO_SECONDS, || fixture.platform.injected_msgs().len() >= 7).await;
    // injected_events() records the mouse-only InputEvent compatibility API. The current HID log
    // injected_msgs is the contract that can represent keyboard and text.
    assert_eq!(
        &fixture.platform.injected_msgs()[..7],
        &[
            InputMsg::Key {
                usage: 0x04,
                down: true,
                mods: Mods::NONE
            },
            InputMsg::Key {
                usage: 0x04,
                down: false,
                mods: Mods::NONE
            },
            InputMsg::Key {
                usage: 0xe3,
                down: true,
                mods: Mods::LMETA
            },
            InputMsg::Key {
                usage: 0x2b,
                down: true,
                mods: Mods::LMETA
            },
            InputMsg::Key {
                usage: 0x2b,
                down: false,
                mods: Mods::LMETA
            },
            InputMsg::Key {
                usage: 0xe3,
                down: false,
                mods: Mods::NONE
            },
            InputMsg::Text("你好".into()),
        ]
    );
    let large = format!("{}🙂{}", "a".repeat(MAX_TEXT_BYTES - 1), "你".repeat(2000));
    sessions::text(&fixture.core, entry.id, "").unwrap();
    sessions::text(&fixture.core, entry.id, &large).unwrap();
    until(TWO_SECONDS, || {
        fixture
            .platform
            .injected_msgs()
            .iter()
            .skip(7)
            .filter_map(|msg| {
                if let InputMsg::Text(text) = msg {
                    Some(text.len())
                } else {
                    None
                }
            })
            .sum::<usize>()
            == large.len()
    })
    .await;
    let chunks: Vec<_> = fixture
        .platform
        .injected_msgs()
        .into_iter()
        .skip(7)
        .map(|msg| {
            let InputMsg::Text(text) = msg else {
                panic!("only text chunks expected");
            };
            assert!(!text.is_empty() && text.len() <= MAX_TEXT_BYTES);
            text
        })
        .collect();
    assert_eq!(chunks.concat(), large);
    fixture.close(entry.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn background_releases_input_pauses_and_resume_starts_with_keyframe() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(OpenIntent::Control).await;
    let recorder = Arc::new(VideoRecorder::default());
    assert!(entry.video.attach(recorder.clone()));
    until(SECOND, || recorder.frame_count() >= 2).await;
    let releases = fixture.platform.release_count();
    sessions::background(&fixture.core, entry.id, true).unwrap();
    sleep(Duration::from_millis(300)).await;
    assert!(fixture.platform.release_count() > releases);
    assert!(fixture
        .platform
        .injected_msgs()
        .contains(&InputMsg::ReleaseAll));
    let count = recorder.frame_count();
    sleep(Duration::from_millis(500)).await;
    assert_eq!(recorder.frame_count(), count);
    recorder.clear();
    sessions::background(&fixture.core, entry.id, false).unwrap();
    until(SECOND, || recorder.frame_count() > 0).await;
    let frame = recorder
        .events()
        .into_iter()
        .find(|event| matches!(event, VideoEvent::Frame { .. }))
        .unwrap();
    assert!(matches!(frame, VideoEvent::Frame { keyframe: true }));
    fixture.close(entry.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn close_ends_native_and_js_streams_removes_summary_and_is_idempotent() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(OpenIntent::View).await;
    let recorder = Arc::new(VideoRecorder::default());
    assert!(entry.video.attach(recorder.clone()));
    fixture.close(entry.id).await;
    until(TWO_SECONDS, || {
        terminal(&fixture.events, entry.id) && user_closed(&fixture.events, entry.id)
    })
    .await;
    assert!(recorder
        .events()
        .iter()
        .any(|event| matches!(event, VideoEvent::Closed(reason) if !reason.is_empty())));
    assert!(sessions::list(&fixture.core).is_empty());
    assert_eq!(entry.summary().phase, SessionPhase::Ended);
    assert!(!entry.video.attach(recorder));
    sessions::close(&fixture.core, entry.id);
    assert_eq!(
        sessions::key(&fixture.core, entry.id, 0x04, true, 0),
        Err(CoreError::internal("unknown session 1"))
    );
    for result in [
        sessions::set_quality(&fixture.core, entry.id, "invalid"),
        sessions::action(&fixture.core, entry.id, "invalid", None),
        sessions::combo(&fixture.core, entry.id, "invalid"),
        sessions::clipboard_send(&fixture.core, entry.id, ""),
    ] {
        assert_eq!(result, Err(CoreError::internal("unknown session 1")));
    }
    assert_eq!(
        fixture
            .events
            .of_type("session.state")
            .iter()
            .filter(|event| event["phase"] == "ended")
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn unpaired_host_ends_before_any_dial_can_time_out() {
    let (core, events) = new_core();
    let started = Instant::now();
    let id = sessions::open(&core, "unpaired", opts(OpenIntent::View)).unwrap();
    assert_eq!(id, 1);
    until(Duration::from_millis(100), || {
        terminal(&events, id) && core.state::<SessionTable>().is_empty()
    })
    .await;
    // Acceptance option: the complete failure, rather than only open(), <100ms.
    assert!(started.elapsed() < Duration::from_millis(100));
    let errors = events.of_type("session.error");
    assert_eq!(errors[0]["code"], 181);
    assert_eq!(errors[0]["hostId"], "unpaired");
    core.shutdown();
}

pub fn blackhole(core: &Arc<Core>) -> UdpSocket {
    let socket = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    pair_row(core, "blackhole", [7; 32], socket.local_addr().unwrap());
    socket
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn unreachable_paired_host_reports_numeric_error_and_ends() {
    let (core, events) = new_core();
    let _socket = blackhole(&core);
    let id = sessions::open(&core, "blackhole", opts(OpenIntent::View)).unwrap();
    until(Duration::from_secs(5), || {
        terminal(&events, id) && core.state::<SessionTable>().is_empty()
    })
    .await;
    let errors = events.of_type("session.error");
    assert_eq!(errors.len(), 1);
    assert!(errors[0]["code"].is_number());
    assert!(errors[0].get("reason").is_none());
    core.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn cancellation_interrupts_dial_and_connecting_commands_are_ignored() {
    let (core, events) = new_core();
    let _socket = blackhole(&core);
    let id = sessions::open(&core, "blackhole", opts(OpenIntent::Control)).unwrap();
    let entry = core.state::<SessionTable>().get(id).unwrap();
    assert!(entry.handle().is_none());
    sessions::key(&core, id, 4, true, 0).unwrap();
    sessions::text(&core, id, "ignored").unwrap();
    sessions::combo(&core, id, "cmdTab").unwrap();
    sessions::set_quality(&core, id, "eco").unwrap();
    sessions::select_display(&core, id, 1).unwrap();
    sessions::request_control(&core, id).unwrap();
    sessions::respond_control(&core, id, "requester", true).unwrap();
    sessions::action(&core, id, "lockOnEnd", None).unwrap();
    sessions::background(&core, id, true).unwrap();
    sessions::background(&core, id, false).unwrap();
    sleep(Duration::from_millis(200)).await;
    sessions::close(&core, id);
    until(Duration::from_millis(500), || {
        terminal(&events, id) && user_closed(&events, id) && core.state::<SessionTable>().is_empty()
    })
    .await;
    assert_eq!(events.of_type("session.error")[0]["message"], "cancelled");
    core.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn commands_validate_forward_quality_display_actions_and_control() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(OpenIntent::Control).await;
    assert_eq!(
        sessions::clipboard_send(&fixture.core, entry.id, ""),
        Err(CoreError::unsupported("clipboard text must not be empty"))
    );
    assert!(matches!(
        sessions::set_quality(&fixture.core, entry.id, "ultra"),
        Err(CoreError::Internal { .. })
    ));
    assert!(matches!(
        sessions::action(&fixture.core, entry.id, "invalid", None),
        Err(CoreError::Internal { .. })
    ));
    assert!(matches!(
        sessions::combo(&fixture.core, entry.id, "invalid"),
        Err(CoreError::Internal { .. })
    ));
    sessions::action(&fixture.core, entry.id, "lockOnEnd", Some(false)).unwrap();
    sessions::action(&fixture.core, entry.id, "lockOnEnd", None).unwrap();
    sessions::set_quality(&fixture.core, entry.id, "eco").unwrap();
    sessions::select_display(&fixture.core, entry.id, 2).unwrap();
    until(TWO_SECONDS, || entry.summary().display_id == Some(2)).await;
    sessions::request_control(&fixture.core, entry.id).unwrap();
    sessions::respond_control(&fixture.core, entry.id, "requester", false).unwrap();
    fixture.close(entry.id).await;
    until(SECOND, || {
        fixture
            .platform
            .session_actions()
            .contains(&pb::session_action::Kind::LockScreen)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn clipboard_send_emits_to_host_and_host_clipboard_emits_from_host() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(OpenIntent::Control).await;

    sessions::clipboard_send(&fixture.core, entry.id, "RS-MOBILE-TO-HOST").unwrap();
    until(TWO_SECONDS, || {
        fixture
            .platform
            .clipboard_writes()
            .iter()
            .any(|item| item.data == b"RS-MOBILE-TO-HOST")
    })
    .await;
    until(TWO_SECONDS, || {
        fixture
            .events
            .of_type("session.clipboard")
            .iter()
            .any(|event| event["direction"] == "toHost" && event["text"] == "RS-MOBILE-TO-HOST")
    })
    .await;

    let data = b"RS-MOBILE-FROM-HOST".to_vec();
    fixture.platform.set_clipboard(Some(PlatformClipboardItem {
        kind: pb::clipboard_header::Kind::Text,
        sha256: host_core::session::clipboard::sha256(&data),
        data,
    }));
    until(TWO_SECONDS, || {
        fixture
            .events
            .of_type("session.clipboard")
            .iter()
            .any(|event| event["direction"] == "fromHost" && event["text"] == "RS-MOBILE-FROM-HOST")
    })
    .await;
    fixture.close(entry.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn prewarm_reuses_tls_and_respects_disabled_and_missing_identity() {
    let fixture = Fixture::new().await;
    sessions::prewarm(&fixture.core, vec![fixture.host.host_id.clone()]);
    until(TWO_SECONDS, || {
        fixture.core.state::<PrewarmPool>().len() == 1
    })
    .await;
    let entry = fixture.streaming(OpenIntent::View).await;
    assert!(entry.handle().unwrap().handshake_timings().reused);
    assert!(fixture.core.state::<PrewarmPool>().is_empty());
    fixture.close(entry.id).await;
    fixture
        .core
        .state::<SettingsState>()
        .0
        .lock()
        .unwrap()
        .prewarm_enabled = false;
    sessions::prewarm(&fixture.core, vec![fixture.host.host_id.clone()]);
    sleep(Duration::from_millis(50)).await;
    assert!(fixture.core.state::<PrewarmPool>().is_empty());
    let no_identity = support::test_core_with(|_, _| {});
    sessions::prewarm(&no_identity, vec![fixture.host.host_id.clone()]);
    assert!(matches!(
        sessions::open(&no_identity, &fixture.host.host_id, opts(OpenIntent::View)),
        Err(CoreError::Internal { .. })
    ));
    assert!(no_identity.state::<SessionTable>().is_empty());
    assert_eq!(no_identity.events().last_seq(), 0);
    no_identity.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
pub async fn cancelled_before_worker_start_is_not_lost_and_params_use_settings() {
    let fixture = Fixture::new().await;
    fixture
        .core
        .state::<SettingsState>()
        .0
        .lock()
        .unwrap()
        .quality_default = Quality::Eco;
    let entry = fixture.core.state::<SessionTable>().insert(
        &fixture.host.host_id,
        "Host",
        opts(OpenIntent::Control),
        fixture.core.now_us(),
    );
    let params = sessions::session_params(&fixture.core, &entry).unwrap();
    assert_eq!(params.intent, pb::Intent::Control);
    assert_eq!(params.quality, pb::Quality::Eco);
    assert_eq!(params.platform, pb::Platform::Ios);
    assert_eq!(params.device_name, "Session test device");
    assert!(params.resume_token.is_none());
    entry.cancel();
    sessions::run_session(&fixture.core, entry.clone()).await;
    until(SECOND, || user_closed(&fixture.events, entry.id)).await;
    assert!(entry.handle().is_none());
    assert!(entry.video.is_closed());
    assert!(sessions::list(&fixture.core).is_empty());
    let mut explicit = opts(OpenIntent::View);
    explicit.quality = Some(Quality::Clear);
    explicit.display_id = Some(2);
    let entry = fixture.core.state::<SessionTable>().insert(
        &fixture.host.host_id,
        "Host",
        explicit,
        fixture.core.now_us(),
    );
    let params = sessions::session_params(&fixture.core, &entry).unwrap();
    assert_eq!(params.quality, pb::Quality::Clear);
    assert_eq!(params.display_id, Some(2));
    assert_eq!(params.intent, pb::Intent::View);
    fixture.core.state::<SessionTable>().remove(entry.id);
}
