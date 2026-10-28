#[path = "update_support/mod.rs"]
mod support;

use std::fs;
use std::sync::Arc;

use host_core::audit::AuditQueue;
use host_core::update::prepare::{
    add_skipped_version, skipped_versions, Downloader, FailReason, PackageVerifier, PrepareContext,
    PrepareDeps, PrepareOutcome, Prepared, Preparer, ReleaseSource, Trigger,
};
use host_core::update::status::{Phase, StatusTx, UpdateStatus};
use host_core::update::types::{Arch, Channel, HeartbeatUpdate, HostOs, UpdateState};
use serde_json::{json, Value};
use support::{
    empty_trusted, signed_manifest, status_channel, FakeDownloader, FakeSource, FakeVerifier,
    SourceReply, TestClock, TestDir,
};
use tokio::sync::watch;
use tokio::time::Instant;

const SEED: [u8; 32] = [7; 32];
const OTHER_SEED: [u8; 32] = [9; 32];
const PACKAGE: &[u8] = b"signed update package contents";
const CURRENT: &str = "0.3.1";
const NEXT: &str = "0.3.2";

struct Harness {
    dir: TestDir,
    source: Arc<FakeSource>,
    downloader: Arc<FakeDownloader>,
    verifier: Arc<FakeVerifier>,
    audit: Option<AuditQueue>,
    status_tx: StatusTx,
    status_rx: watch::Receiver<UpdateStatus>,
    keys: host_core::update::manifest::TrustedKeys,
    max_bytes: u64,
}

impl Harness {
    fn new(manifest: Option<Vec<u8>>, package: &[u8]) -> Self {
        let dir = TestDir::new();
        let source = Arc::new(match manifest {
            Some(raw) => FakeSource::bytes(raw),
            None => FakeSource::new(SourceReply::None),
        });
        let downloader = Arc::new(FakeDownloader::new(package.to_vec()));
        let verifier = Arc::new(FakeVerifier::new());
        let (status_tx, status_rx) = status_channel();
        Self {
            dir,
            source,
            downloader,
            verifier,
            audit: None,
            status_tx: Arc::new(status_tx),
            status_rx,
            keys: support::trusted(SEED),
            max_bytes: 1024 * 1024,
        }
    }

    fn signed(package: &[u8], overrides: Value) -> Self {
        let raw = signed_manifest(SEED, NEXT, package, overrides);
        Self::new(Some(raw), package)
    }

    fn with_audit(mut self) -> Self {
        self.audit = Some(AuditQueue::open(self.dir.path()).expect("open audit queue"));
        self
    }

    fn preparer(&self) -> Preparer {
        Preparer::new(
            PrepareContext {
                current_version: CURRENT.to_owned(),
                os: HostOs::Macos,
                arch: Arch::Universal,
                work_dir: self.dir.path().join("update"),
                keys: self.keys.clone(),
                max_package_bytes: self.max_bytes,
            },
            PrepareDeps {
                source: Arc::clone(&self.source) as Arc<dyn ReleaseSource>,
                downloader: Arc::clone(&self.downloader) as Arc<dyn Downloader>,
                verifier: Arc::clone(&self.verifier) as Arc<dyn PackageVerifier>,
                audit: self.audit.clone(),
                clock: Arc::new(TestClock::new()),
                status: Arc::clone(&self.status_tx),
            },
        )
    }

    async fn prepare(&self, trigger: &Trigger) -> (PrepareOutcome, Vec<Phase>) {
        let phases = collect_phases(self.status_rx.clone());
        let outcome = self.preparer().prepare(trigger, Channel::Stable).await;
        tokio::task::yield_now().await;
        let seen = phases.lock().unwrap().clone();
        (outcome, seen)
    }

    fn status(&self) -> UpdateStatus {
        self.status_rx.borrow().clone()
    }

    fn package_path(&self) -> std::path::PathBuf {
        self.dir
            .path()
            .join("update")
            .join("download")
            .join(NEXT)
            .join("package.dmg")
    }
}

fn collect_phases(mut rx: watch::Receiver<UpdateStatus>) -> Arc<std::sync::Mutex<Vec<Phase>>> {
    let phases = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&phases);
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            sink.lock().unwrap().push(rx.borrow().phase);
        }
    });
    phases
}

fn prepared(outcome: PrepareOutcome) -> Prepared {
    match outcome {
        PrepareOutcome::Prepared(prepared) => prepared,
        other => panic!("expected Prepared, got {other:?}"),
    }
}

fn failed(outcome: PrepareOutcome, reason: FailReason) {
    assert_eq!(outcome, PrepareOutcome::Failed(reason));
}

fn audit_events(queue: &AuditQueue) -> Vec<host_core::audit::AuditEvent> {
    queue
        .batch(100)
        .expect("audit batch")
        .items
        .into_iter()
        .map(|item| item.record)
        .collect()
}

fn assert_no_package(harness: &Harness) {
    assert!(!harness.package_path().exists());
    assert!(!harness.package_path().with_extension("dmg.part").exists());
}

#[tokio::test(start_paused = true)]
async fn periodic_success_downloads_and_verifies_package() {
    let harness = Harness::signed(PACKAGE, json!({}));
    let directory = harness.dir.path().to_path_buf();
    let (outcome, phases) = harness.prepare(&Trigger::Periodic).await;
    assert_eq!(prepared(outcome).package, harness.package_path());
    assert_eq!(fs::read(harness.package_path()).unwrap(), PACKAGE);
    assert!(!harness.package_path().with_extension("dmg.part").exists());
    let start = phases
        .iter()
        .position(|phase| *phase == Phase::Checking)
        .expect("checking");
    let downloading = phases
        .iter()
        .position(|phase| *phase == Phase::Downloading)
        .expect("downloading");
    let verifying = phases
        .iter()
        .position(|phase| *phase == Phase::Verifying)
        .expect("verifying");
    assert!(start < downloading && downloading < verifying, "{phases:?}");
    assert_eq!(harness.status().phase, Phase::Verifying);
    assert_eq!(harness.source.froms(), vec![CURRENT.to_owned()]);
    assert_eq!(harness.verifier.calls(), 1);
    assert_eq!(harness.downloader.max_bytes(), vec![harness.max_bytes]);
    assert!(directory.join("update/download").is_dir());
}

#[tokio::test(start_paused = true)]
async fn no_release_is_idle() {
    let harness = Harness::new(None, PACKAGE);
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    assert_eq!(outcome, PrepareOutcome::NoUpdate);
    assert_eq!(harness.status(), UpdateStatus::idle(Channel::Stable));
}

#[tokio::test(start_paused = true)]
async fn fetch_error_is_idle() {
    let harness = Harness::new(None, PACKAGE);
    harness
        .source
        .queue(SourceReply::Error("network down".to_owned()));
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    failed(outcome, FailReason::Fetch);
    assert_eq!(harness.status(), UpdateStatus::idle(Channel::Stable));
    assert_eq!(harness.downloader.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn tampered_url_fails_signature_and_is_audited() {
    let raw = signed_manifest(SEED, NEXT, PACKAGE, json!({}));
    let mut value: Value = serde_json::from_slice(&raw).unwrap();
    value["url"] = json!("https://evil.example.com/RemoteHost-0.3.2.dmg");
    let harness = Harness::new(Some(serde_json::to_vec(&value).unwrap()), PACKAGE).with_audit();
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    failed(outcome, FailReason::Signature);
    assert_eq!(
        harness.status(),
        UpdateStatus {
            phase: Phase::Failed,
            channel: Channel::Stable,
            pending: None,
            notes: None,
            error: Some("更新清单签名无效".to_owned()),
        }
    );
    assert_eq!(harness.downloader.calls(), 0);
    let audit = audit_events(harness.audit.as_ref().unwrap());
    assert_eq!(audit.len(), 1);
    assert_eq!(
        audit[0].detail,
        Some(json!({"phase": "rejected", "version": null, "reason": "signature"}))
    );
}

#[tokio::test(start_paused = true)]
async fn untrusted_key_fails_signature() {
    let raw = signed_manifest(OTHER_SEED, NEXT, PACKAGE, json!({}));
    let harness = Harness::new(Some(raw), PACKAGE);
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    failed(outcome, FailReason::Signature);
    assert_eq!(harness.downloader.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn non_newer_manifest_is_not_applicable() {
    let raw = signed_manifest(SEED, CURRENT, PACKAGE, json!({}));
    let harness = Harness::new(Some(raw), PACKAGE);
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    failed(outcome, FailReason::NotApplicable);
    assert_eq!(harness.status(), UpdateStatus::idle(Channel::Stable));
    assert_eq!(harness.downloader.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn empty_trust_store_disables_updates_without_fetching() {
    let mut harness = Harness::signed(PACKAGE, json!({}));
    harness.keys = empty_trusted();
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    assert_eq!(outcome, PrepareOutcome::Disabled);
    assert_eq!(harness.source.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn sha256_mismatch_removes_download_and_is_audited() {
    let raw = signed_manifest(SEED, NEXT, PACKAGE, json!({}));
    let mut wrong = PACKAGE.to_vec();
    wrong[0] ^= 1;
    let harness = Harness::new(Some(raw), &wrong).with_audit();
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    failed(outcome, FailReason::Sha256);
    assert_no_package(&harness);
    assert_eq!(harness.status().phase, Phase::Failed);
    let audit = audit_events(harness.audit.as_ref().unwrap());
    assert_eq!(
        audit[0].detail,
        Some(json!({"phase": "rejected", "version": NEXT, "reason": "sha256"}))
    );
}

#[tokio::test(start_paused = true)]
async fn size_mismatch_is_reported() {
    let raw = signed_manifest(SEED, NEXT, PACKAGE, json!({}));
    let mut oversized = PACKAGE.to_vec();
    oversized.push(0);
    let harness = Harness::new(Some(raw), &oversized);
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    failed(outcome, FailReason::Size);
    assert_no_package(&harness);
}

#[tokio::test(start_paused = true)]
async fn oversized_manifest_is_rejected_before_download() {
    let harness = Harness::signed(PACKAGE, json!({}));
    let mut harness = harness;
    harness.max_bytes = PACKAGE.len() as u64 - 1;
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    failed(outcome, FailReason::Size);
    assert_eq!(harness.downloader.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn platform_signature_failure_deletes_package_and_is_audited() {
    let harness = Harness::signed(PACKAGE, json!({})).with_audit();
    harness.verifier.fail("Team ID mismatch");
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    failed(outcome, FailReason::PlatformSignature);
    assert!(!harness.package_path().exists());
    let audit = audit_events(harness.audit.as_ref().unwrap());
    assert_eq!(
        audit[0].detail,
        Some(json!({"phase": "rejected", "version": NEXT, "reason": "platform_signature"}))
    );
}

#[tokio::test(start_paused = true)]
async fn download_retries_twice_then_succeeds() {
    let harness = Harness::signed(PACKAGE, json!({}));
    harness.downloader.fail_first(2);
    let started = Instant::now();
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    assert_eq!(prepared(outcome).package, harness.package_path());
    assert_eq!(harness.downloader.calls(), 3);
    assert_eq!(started.elapsed(), std::time::Duration::from_secs(40));
}

#[tokio::test(start_paused = true)]
async fn download_exhaustion_fails_and_is_audited() {
    let harness = Harness::signed(PACKAGE, json!({})).with_audit();
    harness.downloader.fail_first(3);
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    failed(outcome, FailReason::Download);
    assert_eq!(harness.downloader.calls(), 3);
    assert_no_package(&harness);
    let audit = audit_events(harness.audit.as_ref().unwrap());
    assert_eq!(
        audit[0].detail,
        Some(json!({"phase": "failed", "version": NEXT, "reason": "download"}))
    );
}

#[tokio::test(start_paused = true)]
async fn verified_existing_package_is_reused_without_download() {
    let harness = Harness::signed(PACKAGE, json!({}));
    fs::create_dir_all(harness.package_path().parent().unwrap()).unwrap();
    fs::write(harness.package_path(), PACKAGE).unwrap();
    fs::write(
        harness.package_path().with_extension("dmg.part"),
        b"stale partial download",
    )
    .unwrap();
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    assert_eq!(prepared(outcome).package, harness.package_path());
    assert_eq!(harness.downloader.calls(), 0);
    assert!(!harness.package_path().with_extension("dmg.part").exists());
}

#[tokio::test(start_paused = true)]
async fn periodic_respects_skip_list() {
    let harness = Harness::signed(PACKAGE, json!({}));
    let work_dir = harness.dir.path().join("update");
    add_skipped_version(&work_dir, NEXT).unwrap();
    add_skipped_version(&work_dir, NEXT).unwrap();
    assert_eq!(skipped_versions(&work_dir), vec![NEXT.to_owned()]);

    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    assert_eq!(
        outcome,
        PrepareOutcome::Skipped {
            version: NEXT.to_owned()
        }
    );
    assert_eq!(harness.source.calls(), 1);
    assert_eq!(harness.downloader.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn corrupt_skip_list_is_treated_as_empty() {
    let harness = Harness::signed(PACKAGE, json!({}));
    let work_dir = harness.dir.path().join("update");
    fs::create_dir_all(&work_dir).unwrap();
    fs::write(work_dir.join("skip_versions.json"), b"{not json").unwrap();
    assert!(skipped_versions(&work_dir).is_empty());
    let (outcome, _) = harness.prepare(&Trigger::Periodic).await;
    assert_eq!(prepared(outcome).package, harness.package_path());
}

#[tokio::test(start_paused = true)]
async fn prepare_clears_previous_failure_before_networking() {
    let harness = Harness::signed(PACKAGE, json!({}));
    harness.status_tx.send_replace(UpdateStatus {
        phase: Phase::Failed,
        channel: Channel::Stable,
        pending: Some("0.3.0".to_owned()),
        notes: None,
        error: Some("旧错误".to_owned()),
    });
    let (outcome, phases) = harness.prepare(&Trigger::Periodic).await;
    assert_eq!(prepared(outcome).package, harness.package_path());
    assert_eq!(phases.first(), Some(&Phase::Checking));
}

#[test]
fn status_mapping_is_table_driven() {
    let pending = Some(NEXT.to_owned());
    let table: [(Phase, &str, UpdateState); 11] = [
        (Phase::Idle, "idle", UpdateState::Idle),
        (Phase::Checking, "checking", UpdateState::Idle),
        (Phase::Downloading, "downloading", UpdateState::Downloading),
        (Phase::Verifying, "verifying", UpdateState::Verifying),
        (Phase::Ready, "ready", UpdateState::Idle),
        (
            Phase::WaitingSessionEnd,
            "waiting_session_end",
            UpdateState::WaitingSessionEnd,
        ),
        (Phase::Installing, "installing", UpdateState::Installing),
        (Phase::Restarting, "restarting", UpdateState::Restarting),
        (Phase::Updated, "idle", UpdateState::Ready),
        (Phase::RolledBack, "failed", UpdateState::RolledBack),
        (Phase::Failed, "failed", UpdateState::Failed),
    ];
    for (phase, state, heartbeat) in table {
        let update = UpdateStatus {
            phase,
            channel: Channel::Beta,
            pending: pending.clone(),
            notes: Some("notes".to_owned()),
            error: (phase == Phase::Failed).then(|| "原因".to_owned()),
        };
        assert_eq!(update.ipc_state(), state);
        let actual: HeartbeatUpdate = update.to_heartbeat();
        assert_eq!(actual.channel, Channel::Beta);
        assert_eq!(actual.pending, pending);
        assert_eq!(actual.state, heartbeat);
    }
}
