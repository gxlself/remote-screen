mod support;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rs_mobile::rscore::dto_core::{CoreConfig, NetPath};
use rs_mobile::rscore::dto_files::{
    FileSourceRef, FileTransfer, FileTransferDirection, FileTransferFail, FileTransferState,
};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::{Core, CoreOptions};
use rs_mobile::rscore::xfer::{self, SourceKind, TransferDriver, Transfers};
use serde_json::{json, Value};

use support::EventCollector;

const WAIT: Duration = Duration::from_secs(2);

struct Clock(AtomicU64);

impl Clock {
    fn new(now_us: u64) -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(now_us)))
    }

    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    fn advance(&self, delta_us: u64) {
        self.0.fetch_add(delta_us, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Pause,
    Resume,
    Cancel,
}

#[derive(Default)]
struct FakeDriver {
    calls: Mutex<Vec<Call>>,
    core: Mutex<Option<Arc<Core>>>,
}

impl FakeDriver {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl TransferDriver for FakeDriver {
    fn pause(&self, core: &Arc<Core>, _id: &str) {
        *self.core.lock().unwrap() = Some(core.clone());
        self.calls.lock().unwrap().push(Call::Pause);
    }

    fn cancel(&self, core: &Arc<Core>, _id: &str) {
        *self.core.lock().unwrap() = Some(core.clone());
        self.calls.lock().unwrap().push(Call::Cancel);
    }

    async fn resume(&self, core: &Arc<Core>, _id: &str) -> Result<(), CoreError> {
        *self.core.lock().unwrap() = Some(core.clone());
        self.calls.lock().unwrap().push(Call::Resume);
        Ok(())
    }
}

struct Fixture {
    core: Arc<Core>,
    clock: Arc<Clock>,
    events: Arc<EventCollector>,
    driver: Arc<FakeDriver>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_dir(None)
    }

    fn with_dir(data_dir: Option<&Path>) -> Self {
        let clock = Clock::new(1_000_000);
        let clock_for_core = clock.clone();
        let directory = data_dir.map(Path::to_path_buf);
        let core = support::test_core_with(|cfg, opts| {
            if let Some(directory) = directory {
                cfg.data_dir = directory.to_string_lossy().into_owned();
            }
            opts.clock = Some(Arc::new(move || clock_for_core.now()));
        });
        let events = EventCollector::new();
        core.events().subscribe(events.clone());
        let driver = Arc::new(FakeDriver::default());
        core.state::<Transfers>()
            .register_driver(SourceKind::Computer, driver.clone());
        Self {
            core,
            clock,
            events,
            driver,
        }
    }

    fn transfer(&self, id: &str, state: FileTransferState) -> FileTransfer {
        transfer(id, state, self.clock.now())
    }

    fn create(&self, transfer: FileTransfer) {
        self.core.state::<Transfers>().create(&self.core, transfer);
    }

    fn flush(&self) -> Vec<Value> {
        assert!(self.core.events().flush(WAIT));
        self.events
            .events()
            .into_iter()
            .map(|(_, value)| value)
            .collect()
    }
}

fn transfer(id: &str, state: FileTransferState, started_at: u64) -> FileTransfer {
    FileTransfer {
        transfer_id: id.to_owned(),
        direction: FileTransferDirection::Download,
        source: FileSourceRef::Computer {
            host_id: "host-1".to_owned(),
        },
        name: format!("{id}.bin"),
        remote_path: Some(format!("/remote/{id}.bin")),
        local_path: None,
        size: 3 * 1024 * 1024,
        done: 0,
        bytes_per_sec: 0,
        state,
        fail: None,
        net_path: Some(NetPath::Lan),
        saved_name: None,
        started_at,
        updated_at: started_at,
    }
}

fn reopen(data_dir: &Path) -> Arc<Core> {
    let data_dir = data_dir.to_string_lossy().into_owned();
    Core::new(
        CoreConfig {
            files_dir: None,
            data_dir,
            app_version: "test".to_owned(),
            platform: rs_mobile::rscore::dto_core::DevicePlatform::Ios,
        },
        CoreOptions {
            init_logging: false,
            ..CoreOptions::default()
        },
    )
    .unwrap()
}

fn assert_http(error: CoreError, status: u16, prefix: &str) {
    match error {
        CoreError::Http {
            status: actual,
            message,
            ..
        } => {
            assert_eq!(actual, status);
            assert!(message.starts_with(prefix), "message was {message:?}");
        }
        other => panic!("expected HTTP {status}, got {other:?}"),
    }
}

#[test]
fn create_emits_camel_case_transfer_and_lists_it() {
    let fixture = Fixture::new();
    let original = fixture.transfer("t-create", FileTransferState::Running);
    fixture.create(original.clone());

    let listed = fixture.core.state::<Transfers>().list(&fixture.core);
    assert_eq!(listed, std::slice::from_ref(&original));
    let events = fixture.flush();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], "file.transfer");
    assert_eq!(events[0]["transfer"]["transferId"], original.transfer_id);
    assert_eq!(events[0]["transfer"]["bytesPerSec"], 0);
    assert_eq!(events[0]["transfer"]["startedAt"], original.started_at);
}

#[test]
fn create_reusing_an_id_refreshes_time_without_resetting_throttle() {
    let fixture = Fixture::new();
    let mut item = fixture.transfer("t-reuse", FileTransferState::Running);
    item.done = 10;
    fixture.create(item.clone());
    fixture.flush();

    fixture.clock.advance(100_000);
    item.done = 20;
    fixture.create(item);
    let created = fixture
        .core
        .state::<Transfers>()
        .get(&fixture.core, "t-reuse")
        .unwrap();
    assert_eq!(created.done, 20);
    assert_eq!(created.updated_at, fixture.clock.now());

    fixture.clock.advance(100_000);
    fixture
        .core
        .state::<Transfers>()
        .progress(&fixture.core, "t-reuse", 30);
    assert_eq!(fixture.flush().len(), 2);
}

#[test]
fn progress_is_throttled_and_always_reports_completion() {
    let fixture = Fixture::new();
    let mut item = fixture.transfer("t-progress", FileTransferState::Running);
    item.size = 1_000_000;
    fixture.create(item);
    let baseline = fixture.flush().len();

    for step in 1..=10 {
        fixture.clock.advance(50_000);
        let done = if step == 10 { 1_000_000 } else { step * 10_000 };
        fixture
            .core
            .state::<Transfers>()
            .progress(&fixture.core, "t-progress", done);
    }
    let events: Vec<Value> = fixture
        .flush()
        .into_iter()
        .filter(|event| event["type"] == "file.transfer")
        .collect();
    let progress_events = events.len() - baseline;
    assert!(
        (2..=3).contains(&progress_events),
        "expected 2-3 throttled events, got {progress_events}"
    );
    assert_eq!(events.last().unwrap()["transfer"]["done"], 1_000_000);
}

#[test]
fn speed_uses_a_two_second_window_and_resets_when_paused() {
    let fixture = Fixture::new();
    let mut item = fixture.transfer("t-speed", FileTransferState::Running);
    item.size = 2 * 1024 * 1024;
    fixture.create(item);
    fixture.clock.advance(1_000_000);
    fixture
        .core
        .state::<Transfers>()
        .progress(&fixture.core, "t-speed", 1024 * 1024);
    let current = fixture
        .core
        .state::<Transfers>()
        .get(&fixture.core, "t-speed")
        .unwrap();
    let expected = 1024_u64 * 1024;
    let tolerance = expected / 10;
    assert!(
        current.bytes_per_sec.abs_diff(expected) <= tolerance,
        "speed {} not near {expected}",
        current.bytes_per_sec
    );

    fixture
        .core
        .state::<Transfers>()
        .update(&fixture.core, "t-speed", |item| {
            item.state = FileTransferState::Paused
        });
    let paused = fixture
        .core
        .state::<Transfers>()
        .get(&fixture.core, "t-speed")
        .unwrap();
    assert_eq!(paused.bytes_per_sec, 0);
}

#[test]
fn state_changes_emit_immediately() {
    let fixture = Fixture::new();
    let item = fixture.transfer("t-state", FileTransferState::Running);
    fixture.create(item);
    fixture.flush();
    let baseline = fixture.events.events().len();
    fixture.clock.advance(1);
    let updated = fixture
        .core
        .state::<Transfers>()
        .update(&fixture.core, "t-state", |item| {
            item.state = FileTransferState::Verifying
        });
    assert_eq!(updated.unwrap().state, FileTransferState::Verifying);
    let events = fixture.flush();
    assert_eq!(events.len() - baseline, 1);
    assert_eq!(events.last().unwrap()["transfer"]["state"], "verifying");
}

#[test]
fn state_survives_reopen_and_running_becomes_interrupted() {
    let fixture = Fixture::new();
    let mut running = fixture.transfer("t-running", FileTransferState::Running);
    running.done = 100;
    let mut completed = fixture.transfer("t-completed", FileTransferState::Completed);
    completed.done = completed.size;
    let paused = fixture.transfer("t-paused", FileTransferState::Paused);
    for item in [running, completed, paused] {
        fixture.create(item);
    }

    let reopened = reopen(&fixture.core.data_dir());
    let listed = reopened.state::<Transfers>().list(&reopened);
    assert_eq!(listed.len(), 3);
    let running = listed
        .iter()
        .find(|item| item.transfer_id == "t-running")
        .unwrap();
    assert_eq!(running.state, FileTransferState::Interrupted);
    assert_eq!(running.bytes_per_sec, 0);
    assert_eq!(running.done, 100);
}

#[test]
fn persistence_prunes_oldest_completed_transfers_to_two_hundred() {
    let fixture = Fixture::new();
    for index in 0..205 {
        let mut item = fixture.transfer(&format!("t-{index:03}"), FileTransferState::Completed);
        item.started_at = index;
        item.updated_at = index;
        fixture.create(item);
    }
    let path = fixture.core.data_dir().join("transfers.json");
    let saved: Vec<FileTransfer> =
        serde_json::from_slice(&std::fs::read(path).expect("persisted table")).unwrap();
    assert_eq!(saved.len(), 200);
    assert!(saved.iter().all(|item| item.transfer_id != "t-000"));
    assert!(saved.iter().all(|item| item.transfer_id != "t-004"));
    assert!(saved.iter().any(|item| item.transfer_id == "t-005"));
}

#[test]
fn invalid_json_starts_empty_and_is_preserved_as_bad() {
    let directory = support::temp_dir("xfer-bad");
    std::fs::write(directory.join("transfers.json"), b"not json").unwrap();
    let core = reopen(&directory);
    assert!(core.state::<Transfers>().list(&core).is_empty());
    assert!(directory.join("transfers.json.bad").is_file());
    std::fs::write(directory.join("transfers.json"), b"still not json").unwrap();
    assert!(core.state::<Transfers>().list(&core).is_empty());
}

#[test]
fn update_emits_progress_only_when_the_throttle_allows_it() {
    let fixture = Fixture::new();
    let item = fixture.transfer("t-update-throttle", FileTransferState::Running);
    fixture.create(item);
    let baseline = fixture.flush().len();

    fixture.clock.advance(50_000);
    fixture
        .core
        .state::<Transfers>()
        .update(&fixture.core, "t-update-throttle", |item| item.done = 10);
    assert_eq!(fixture.flush().len(), baseline);

    fixture.clock.advance(200_000);
    fixture
        .core
        .state::<Transfers>()
        .update(&fixture.core, "t-update-throttle", |item| item.done = 20);
    let events = fixture.flush();
    assert_eq!(events.len(), baseline + 1);
    assert_eq!(events.last().unwrap()["transfer"]["done"], 20);
}

#[test]
fn pause_changes_state_and_calls_the_driver() {
    let fixture = Fixture::new();
    let item = fixture.transfer("t-pause", FileTransferState::Running);
    fixture.create(item);
    xfer::pause(&fixture.core, "t-pause").unwrap();
    assert_eq!(
        fixture
            .core
            .state::<Transfers>()
            .get(&fixture.core, "t-pause")
            .unwrap()
            .state,
        FileTransferState::Paused
    );
    assert_eq!(fixture.driver.calls(), [Call::Pause]);

    fixture
        .core
        .state::<Transfers>()
        .update(&fixture.core, "t-pause", |item| {
            item.state = FileTransferState::Completed
        });
    let error = xfer::pause(&fixture.core, "t-pause").unwrap_err();
    assert_http(error, 409, "transfers.notActive:");
    assert_eq!(fixture.driver.calls(), [Call::Pause]);
}

#[test]
fn resume_delegates_to_the_driver_and_rejects_cancelled() {
    let fixture = Fixture::new();
    let mut item = fixture.transfer("t-resume", FileTransferState::Paused);
    fixture.create(item.clone());
    fixture
        .core
        .block_on(xfer::resume(&fixture.core, "t-resume"))
        .unwrap();
    assert_eq!(fixture.driver.calls(), [Call::Resume]);

    item.state = FileTransferState::Cancelled;
    item.transfer_id = "t-cancelled".to_owned();
    fixture.create(item);
    let error = fixture
        .core
        .block_on(xfer::resume(&fixture.core, "t-cancelled"))
        .unwrap_err();
    assert_http(error, 409, "transfers.notResumable:");
}

#[test]
fn cancel_is_idempotent_and_clears_failure() {
    let fixture = Fixture::new();
    let mut item = fixture.transfer("t-cancel", FileTransferState::Running);
    item.fail = Some(FileTransferFail::Io);
    fixture.create(item);
    xfer::cancel(&fixture.core, "t-cancel").unwrap();
    let cancelled = fixture
        .core
        .state::<Transfers>()
        .get(&fixture.core, "t-cancel")
        .unwrap();
    assert_eq!(cancelled.state, FileTransferState::Cancelled);
    assert_eq!(cancelled.fail, None);
    assert_eq!(fixture.driver.calls(), [Call::Cancel]);
    xfer::cancel(&fixture.core, "t-cancel").unwrap();
    assert_eq!(fixture.driver.calls(), [Call::Cancel]);
}

#[test]
fn remove_cancels_active_transfer_and_keeps_the_downloaded_file() {
    let fixture = Fixture::new();
    let local_path = fixture.core.data_dir().join("downloaded.bin");
    std::fs::write(&local_path, b"contents").unwrap();
    let mut item = fixture.transfer("t-remove", FileTransferState::Running);
    item.local_path = Some(local_path.to_string_lossy().into_owned());
    fixture.create(item);
    fixture.flush();
    xfer::remove(&fixture.core, "t-remove").unwrap();
    assert_eq!(fixture.driver.calls(), [Call::Cancel]);
    let events = fixture.flush();
    assert_eq!(events.last().unwrap()["type"], "file.transferRemoved");
    assert_eq!(events.last().unwrap()["transferId"], "t-remove");
    assert!(local_path.is_file());
    assert!(fixture
        .core
        .state::<Transfers>()
        .get(&fixture.core, "t-remove")
        .is_none());
}

#[test]
fn unknown_ids_and_missing_drivers_return_contract_errors() {
    let fixture = Fixture::new();
    let error = xfer::pause(&fixture.core, "missing").unwrap_err();
    assert_http(error, 404, "transfers.notFound:");
    assert_http(
        xfer::cancel(&fixture.core, "missing").unwrap_err(),
        404,
        "transfers.notFound:",
    );
    assert_http(
        xfer::remove(&fixture.core, "missing").unwrap_err(),
        404,
        "transfers.notFound:",
    );

    let mut item = fixture.transfer("t-server", FileTransferState::Running);
    item.source = FileSourceRef::Server {
        server_id: "server-1".to_owned(),
    };
    fixture.create(item);
    assert!(matches!(
        xfer::pause(&fixture.core, "t-server"),
        Err(CoreError::Unsupported { .. })
    ));
}

#[test]
fn list_is_newest_first() {
    let fixture = Fixture::new();
    for (id, started) in [("old", 10), ("new", 30), ("middle", 20)] {
        let mut item = fixture.transfer(id, FileTransferState::Paused);
        item.started_at = started;
        fixture.create(item);
    }
    let ids: Vec<_> = fixture
        .core
        .state::<Transfers>()
        .list(&fixture.core)
        .into_iter()
        .map(|item| item.transfer_id)
        .collect();
    assert_eq!(ids, ["new", "middle", "old"]);
}

#[test]
fn paused_transfer_json_keeps_the_dto_shape() {
    let fixture = Fixture::new();
    let item = fixture.transfer("t-json", FileTransferState::Paused);
    fixture.create(item);
    let events = fixture.flush();
    assert_eq!(
        events[0],
        json!({
            "type": "file.transfer",
            "transfer": {
                "transferId": "t-json", "direction": "download",
                "source": {"kind": "computer", "hostId": "host-1"},
                "name": "t-json.bin", "remotePath": "/remote/t-json.bin",
                "localPath": null, "size": 3_145_728, "done": 0,
                "bytesPerSec": 0, "state": "paused", "fail": null,
                "netPath": "lan", "savedName": null, "startedAt": 1_000_000,
                "updatedAt": 1_000_000
            }
        })
    );
}
