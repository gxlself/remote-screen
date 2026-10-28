mod support;

use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use host_core::clock::Clock;
use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_diag::{refresh_network, set_setting},
    api_sessions as sessions,
    dto_core::{OpenIntent, OpenOpts, SessionPhase},
    netinfo::RawIface,
    settings::Settings,
    store::PairedHostRow,
    tables::{SessionEntry, SessionTable, VideoConsumer},
};
use rsp_client::{CursorEvent, ReceivedFrame};
use rsp_core::{pb, trust::Permission};
use serde_json::{json, Value};
use tokio::time::{sleep, timeout};

fn iface(name: &str, addr: &str) -> RawIface {
    RawIface {
        name: name.into(),
        addrs: vec![addr.parse().unwrap()],
    }
}

fn opts() -> OpenOpts {
    OpenOpts {
        intent: OpenIntent::View,
        display_id: None,
        quality: None,
        mode: None,
        audio: None,
        max_fps: None,
        max_width: None,
        codec: None,
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
struct OffsetClock {
    epoch_us: u64,
    offset_us: AtomicU64,
}

impl OffsetClock {
    fn new() -> Self {
        let epoch_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_micros() as u64);
        Self {
            epoch_us,
            offset_us: AtomicU64::new(0),
        }
    }

    fn skip(&self, duration: Duration) {
        self.offset_us
            .fetch_add(duration.as_micros().try_into().unwrap(), Ordering::Relaxed);
    }
}

impl Clock for OffsetClock {
    fn now_micros(&self) -> u64 {
        self.epoch_us
            .saturating_add(self.offset_us.load(Ordering::Relaxed))
    }
}

#[derive(Default)]
struct MinimalVideoConsumer {
    frames: AtomicUsize,
    configs: AtomicUsize,
    keyframes: AtomicUsize,
}

impl VideoConsumer for MinimalVideoConsumer {
    fn frame(&self, frame: &ReceivedFrame) {
        self.frames.fetch_add(1, Ordering::Relaxed);
        if frame.frame.keyframe {
            self.keyframes.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn stream_config(&self, _: &pb::StreamConfig) {
        self.configs.fetch_add(1, Ordering::Relaxed);
    }

    fn cursor(&self, _: &CursorEvent) {}
    fn cursor_msg(&self, _: &pb::CursorMsg) {}
    fn clock(&self, _: u64, _: i64) {}
    fn closed(&self, _: &str) {}
}

fn pair_row(core: &Arc<rs_mobile::rscore::runtime::Core>, host: &TestHost) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host.host_id.clone(),
            fp: host.fp,
            name: "Device hints test host".into(),
            addresses: vec![host.quic_addr],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();
}

async fn open_streaming(
    core: &Arc<rs_mobile::rscore::runtime::Core>,
    host: &TestHost,
) -> Arc<SessionEntry> {
    let id = sessions::open(core, &host.host_id, opts()).unwrap();
    let entry = core.state::<SessionTable>().get(id).unwrap();
    until(Duration::from_secs(3), || {
        entry.summary().phase == SessionPhase::Streaming && entry.handle().is_some()
    })
    .await;
    entry
}

async fn background_hold_roundtrip(hold: bool) {
    let core = support::test_core_with(|_, options| options.worker_threads = 2);
    support::with_identity(&core, "Device hints test device");
    let clock = Arc::new(OffsetClock::new());
    let host = TestHost::start_with_platform(
        FakePlatform::new(),
        TestHostOptions {
            tcp: false,
            services_clock: Some(clock.clone()),
            ..Default::default()
        },
    )
    .await;
    host.pair(&core.local_identity().unwrap(), Permission::Control);
    pair_row(&core, &host);
    let entry = open_streaming(&core, &host).await;
    let video = Arc::new(MinimalVideoConsumer::default());
    assert!(entry.video.attach(video.clone()));
    until(Duration::from_secs(1), || {
        video.frames.load(Ordering::Acquire) > 2
    })
    .await;
    set_setting(&core, "device.backgroundHold", &hold.to_string()).unwrap();
    sessions::background(&core, entry.id, true).unwrap();
    sleep(Duration::from_millis(300)).await;
    let paused_frames = video.frames.load(Ordering::Acquire);
    sleep(Duration::from_millis(300)).await;
    assert_eq!(video.frames.load(Ordering::Acquire), paused_frames);

    let configs_before = video.configs.load(Ordering::Acquire);
    clock.skip(Duration::from_secs(46));
    until(Duration::from_secs(2), || {
        video.configs.load(Ordering::Acquire) > configs_before
    })
    .await;
    let after_config = video.frames.load(Ordering::Acquire);
    sleep(Duration::from_secs(1)).await;
    let settled = video.frames.load(Ordering::Acquire);
    sleep(Duration::from_millis(500)).await;
    let final_frames = video.frames.load(Ordering::Acquire);
    if hold {
        assert!(settled >= after_config);
        assert_eq!(final_frames, settled);
    } else {
        assert!(settled > paused_frames);
        assert!(final_frames >= settled);
    }

    if hold {
        let keyframes_before = video.keyframes.load(Ordering::Acquire);
        sessions::background(&core, entry.id, false).unwrap();
        until(Duration::from_secs(1), || {
            video.keyframes.load(Ordering::Acquire) > keyframes_before
        })
        .await;
    }
    sessions::close(&core, entry.id);
    until(Duration::from_secs(2), || {
        core.state::<SessionTable>().is_empty()
    })
    .await;
    core.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_hold_repauses_after_host_stream_timeout() {
    background_hold_roundtrip(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_without_hold_keeps_streaming_after_host_timeout() {
    background_hold_roundtrip(false).await;
}

#[test]
fn device_hints_validate_and_normalize() {
    let mut settings = Settings::default();
    assert!(!settings.background_hold);
    assert_eq!(settings.set("device.backgroundHold", json!(true)), Ok(true));
    assert_eq!(
        settings.set("device.backgroundHold", json!("true")),
        Err(rs_mobile::rscore::settings::SettingsError(
            "bad value for device.backgroundHold".into()
        ))
    );
    assert!(settings.background_hold);
    assert_eq!(settings.background_grace(), None);
    settings.set("device.backgroundHold", json!(false)).unwrap();
    assert_eq!(
        settings.background_grace(),
        Some(rsp_client::recovery::background::PAUSE_GRACE)
    );

    assert_eq!(
        settings.set("device.workProfileIfaces", Value::Null),
        Ok(false)
    );
    assert_eq!(
        settings.set("device.workProfileIfaces", json!(["tun1", "tun0", "tun1"])),
        Ok(true)
    );
    assert_eq!(settings.work_profile_ifaces, ["tun0", "tun1"]);
    assert_eq!(
        settings.set("device.workProfileIfaces", json!(["tun1", "tun0"])),
        Ok(false)
    );
    for value in [
        json!("tun0"),
        json!([1]),
        json!([""]),
        json!((0..33).map(|i| format!("tun{i}")).collect::<Vec<_>>()),
        json!(["x".repeat(65)]),
    ] {
        assert!(settings.set("device.workProfileIfaces", value).is_err());
        assert_eq!(settings.work_profile_ifaces, ["tun0", "tun1"]);
    }
}

#[test]
fn work_profile_setting_refreshes_network_status_immediately() {
    let core = support::test_core_with(|_, _| {});
    let events = support::EventCollector::new();
    core.events().subscribe(events.clone());
    let raw = [iface("synthetic-wp", "10.99.4.27/32")];
    refresh_network(&core, &raw);
    assert!(core.events().flush(Duration::from_secs(1)));
    assert!(events.of_type("network.changed").is_empty());

    set_setting(&core, "device.workProfileIfaces", r#"["synthetic-wp"]"#).unwrap();
    assert!(core.events().flush(Duration::from_secs(1)));
    let changed = events.of_type("network.changed");
    assert_eq!(changed.len(), 1);
    let marked = refresh_network(&core, &raw);
    assert_eq!(marked.ifaces[0].work_profile, Some(true));

    set_setting(&core, "device.workProfileIfaces", "[]").unwrap();
    let unmarked = refresh_network(&core, &raw);
    assert_eq!(unmarked.ifaces[0].work_profile, None);
    core.shutdown();
}
