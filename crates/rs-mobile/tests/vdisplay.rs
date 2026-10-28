mod support;

use std::{sync::Arc, time::Duration};

use host_fake::{
    harness::{TestHost, TestHostOptions},
    vdisplay::{FakeVdOptions, FakeVirtualDisplays},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_sessions as sessions,
    dto_core::{CoreEvent, OpenIntent, OpenOpts, SessionPhase, VdisplayStatus},
    error::CoreError,
    runtime::{install_core, take_core, Core},
    store::PairedHostRow,
    tables::SessionTable,
    vdisplay::{create, release, result_event},
};
use rsp_core::{
    pb::{self, virtual_display_result::Status},
    trust::Permission,
};
use serde_json::{json, Value};
use tokio::{
    sync::{Mutex, MutexGuard},
    time::{sleep, timeout},
};

static GLOBAL_CORE: Mutex<()> = Mutex::const_new(());
const TWO_SECONDS: Duration = Duration::from_secs(2);
const SELECTED_OPTS: &str =
    r#"{"widthPx":2732,"heightPx":2048,"scale":2,"refreshHz":60,"select":true}"#;

fn virtual_display() -> pb::Display {
    pb::Display {
        id: 3,
        name: "V".into(),
        width_px: 2732,
        height_px: 2048,
        refresh_hz: 60,
        is_virtual: true,
        ..Default::default()
    }
}

#[test]
fn maps_every_status() {
    for (wire, expected) in [
        (Status::Ok as i32, VdisplayStatus::Ok),
        (Status::Unsupported as i32, VdisplayStatus::Unsupported),
        (Status::NotAllowed as i32, VdisplayStatus::NotAllowed),
        (Status::Limit as i32, VdisplayStatus::Limit),
        (Status::BadRequest as i32, VdisplayStatus::BadRequest),
        (Status::Failed as i32, VdisplayStatus::Failed),
        (0, VdisplayStatus::Failed),
        (99, VdisplayStatus::Failed),
    ] {
        let event = result_event(
            "s1",
            &pb::VirtualDisplayResult {
                request_id: 7,
                status: wire,
                ..Default::default()
            },
        )
        .expect("every result produces an event");
        let CoreEvent::SessionVdisplay {
            session_id,
            request_id,
            status,
            ..
        } = event
        else {
            panic!("expected session.vdisplay");
        };
        assert_eq!(session_id, "s1");
        assert_eq!(request_id, 7);
        assert_eq!(status, expected, "wire status {wire}");
    }
}

#[test]
fn display_only_on_ok() {
    for (wire, has_display) in [
        (Status::Ok as i32, true),
        (Status::Failed as i32, false),
        (0, false),
        (99, false),
    ] {
        let event = result_event(
            "s1",
            &pb::VirtualDisplayResult {
                status: wire,
                display: Some(virtual_display()),
                ..Default::default()
            },
        )
        .unwrap();
        let CoreEvent::SessionVdisplay { display, .. } = event else {
            panic!("expected session.vdisplay");
        };
        assert_eq!(display.is_some(), has_display, "wire status {wire}");
        if let Some(display) = display {
            assert_eq!(display.id, 3);
            assert!(display.is_virtual);
        }
    }
    let event = result_event(
        "s1",
        &pb::VirtualDisplayResult {
            status: Status::Ok as i32,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        event,
        CoreEvent::SessionVdisplay { display: None, .. }
    ));
}

#[test]
fn message_empty_is_none() {
    for (message, expected) in [("", None), ("boom", Some("boom"))] {
        let event = result_event(
            "s1",
            &pb::VirtualDisplayResult {
                message: message.into(),
                ..Default::default()
            },
        )
        .unwrap();
        let CoreEvent::SessionVdisplay { message, .. } = event else {
            panic!("expected session.vdisplay");
        };
        assert_eq!(message.as_deref(), expected);
    }
}

#[test]
fn event_json_shape() {
    let mut result = pb::VirtualDisplayResult {
        request_id: 7,
        status: Status::Ok as i32,
        display: Some(virtual_display()),
        message: String::new(),
    };
    let event = result_event("s1", &result).unwrap();
    assert_eq!(event.event_type(), "session.vdisplay");
    assert_eq!(
        serde_json::to_value(&event).unwrap(),
        json!({
            "type": "session.vdisplay",
            "sessionId": "s1",
            "requestId": 7,
            "status": "ok",
            "display": {
                "id": 3,
                "name": "V",
                "widthPx": 2732,
                "heightPx": 2048,
                "primary": false,
                "refreshHz": 60,
                "isVirtual": true
            }
        })
    );
    result.status = Status::NotAllowed as i32;
    assert_eq!(
        serde_json::to_value(result_event("s1", &result).unwrap()).unwrap(),
        json!({
            "type": "session.vdisplay",
            "sessionId": "s1",
            "requestId": 7,
            "status": "notAllowed"
        })
    );
}

#[test]
fn bad_json_is_internal() {
    let core = support::test_core();
    for opts in ["{", r#"{"widthPx":1,"heightPx":1,"x":1}"#] {
        let error = create(&core, 999, opts).unwrap_err();
        let CoreError::Internal { message } = error else {
            panic!("expected internal error");
        };
        assert!(message.starts_with("invalid virtual display options: "));
    }
    core.shutdown();
}

#[test]
fn unknown_session() {
    let core = support::test_core();
    let expected = sessions::select_display(&core, 999, 3).unwrap_err();
    for opts in [
        r#"{"widthPx":2732,"heightPx":2048}"#,
        r#"{"widthPx":0,"heightPx":0}"#,
    ] {
        assert_eq!(create(&core, 999, opts).unwrap_err(), expected);
    }
    assert_eq!(release(&core, 999, 3).unwrap_err(), expected);
    core.shutdown();
}

async fn until(predicate: impl Fn() -> bool) {
    while !predicate() {
        sleep(Duration::from_millis(5)).await;
    }
}

struct Fixture {
    core: Arc<Core>,
    events: Arc<support::EventCollector>,
    host: TestHost,
    backend: Arc<FakeVirtualDisplays>,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new() -> Self {
        let serial = GLOBAL_CORE.lock().await;
        let core = support::test_core();
        support::with_identity(&core, "Virtual display test device");
        let events = support::EventCollector::new();
        core.events().subscribe(events.clone());
        install_core(core.clone());
        let platform = FakePlatform::new();
        let backend = FakeVirtualDisplays::install(&platform, FakeVdOptions::default());
        let host = TestHost::start_with_platform(
            platform,
            TestHostOptions {
                tcp: false,
                ..Default::default()
            },
        )
        .await;
        host.pair(&core.local_identity().unwrap(), Permission::Control);
        core.store()
            .upsert_paired_host(&PairedHostRow {
                host_id: host.host_id.clone(),
                fp: host.fp,
                name: "Virtual display test host".into(),
                addresses: vec![host.quic_addr],
                paired_at_us: core.now_us(),
                permission: Some("control".into()),
            })
            .unwrap();
        Self {
            core,
            events,
            host,
            backend,
            _serial: serial,
        }
    }

    async fn open(&self) -> u64 {
        let id = sessions::open(
            &self.core,
            &self.host.host_id,
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
        timeout(
            TWO_SECONDS,
            until(|| {
                self.core
                    .state::<SessionTable>()
                    .get(id)
                    .is_some_and(|entry| entry.summary().phase == SessionPhase::Streaming)
            }),
        )
        .await
        .expect("control session streaming within 2 s");
        id
    }

    async fn result(&self, id: u64, request_id: u32) -> Value {
        let session_id = id.to_string();
        loop {
            if let Some(event) = self
                .events
                .of_type("session.vdisplay")
                .into_iter()
                .find(|event| event["sessionId"] == session_id && event["requestId"] == request_id)
            {
                return event;
            }
            sleep(Duration::from_millis(5)).await;
        }
    }

    async fn create_selected(&self, id: u64) -> (u32, u32) {
        let request_id = create(&self.core, id, SELECTED_OPTS).unwrap();
        assert_ne!(request_id, 0);
        let display_id = timeout(TWO_SECONDS, async {
            let result = self.result(id, request_id).await;
            assert_eq!(result["status"], "ok");
            assert_eq!(result["display"]["isVirtual"], true);
            assert_eq!(result["display"]["widthPx"], 2732);
            assert_eq!(result["display"]["heightPx"], 2048);
            assert_eq!(result["display"]["refreshHz"], 60);
            let display_id: u32 = serde_json::from_value(result["display"]["id"].clone()).unwrap();
            until(|| {
                self.events
                    .of_type("session.displays")
                    .last()
                    .is_some_and(|event| {
                        event["sessionId"].as_str() == Some(id.to_string().as_str())
                            && event["currentId"] == display_id
                            && event["displays"].as_array().is_some_and(|displays| {
                                displays.iter().any(|display| {
                                    display["id"] == display_id && display["isVirtual"] == true
                                })
                            })
                    })
            })
            .await;
            display_id
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "virtual display result and selection within 2 s; events: {:?}",
                self.events.events()
            )
        });
        (request_id, display_id)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for entry in self.core.state::<SessionTable>().list() {
            sessions::close(&self.core, entry.id);
        }
        take_core();
        self.core.shutdown();
    }
}

#[tokio::test]
async fn create_emits_ok_and_lists_virtual() {
    let fixture = Fixture::new().await;
    let id = fixture.open().await;
    fixture.create_selected(id).await;
}

#[tokio::test]
async fn release_removes_from_list() {
    let fixture = Fixture::new().await;
    let id = fixture.open().await;
    let (_, display_id) = fixture.create_selected(id).await;
    let before_release = fixture.core.events().last_seq();
    release(&fixture.core, id, display_id).unwrap();
    timeout(
        TWO_SECONDS,
        until(|| {
            fixture.events.events().iter().any(|(seq, event)| {
                *seq > before_release
                    && event["type"] == "session.displays"
                    && event["sessionId"].as_str() == Some(id.to_string().as_str())
                    && event["displays"].as_array().is_some_and(|displays| {
                        displays.iter().all(|display| display["id"] != display_id)
                    })
            }) && fixture.backend.destroyed() == [1]
        }),
    )
    .await
    .expect("released display removed from list and destroyed within 2 s");
    assert_eq!(fixture.backend.destroyed(), [1]);
}

#[tokio::test]
async fn request_ids_increase() {
    let fixture = Fixture::new().await;
    let id = fixture.open().await;
    let (first, display_id) = fixture.create_selected(id).await;
    let second = create(&fixture.core, id, r#"{"widthPx":2732,"heightPx":2048}"#).unwrap();
    assert_ne!(first, 0);
    assert_ne!(second, 0);
    assert!(second > first);
    let result = timeout(TWO_SECONDS, fixture.result(id, second))
        .await
        .expect("second request reaches Host and emits its own result");
    assert_eq!(result["status"], "ok");
    assert_eq!(result["display"]["id"], display_id);
}
