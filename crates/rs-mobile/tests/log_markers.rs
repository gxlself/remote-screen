//! Process-wide logger test: this file intentionally contains one test.

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use host_core::platform::ClipboardItem;
use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rcgen::PublicKeyData;
use ring::{
    rand::SystemRandom,
    signature::{EcdsaKeyPair, ECDSA_P256_SHA256_ASN1_SIGNING},
};
use rs_mobile::rscore::{
    api_sessions as sessions,
    dto_core::{CoreConfig, DevicePlatform, OpenIntent, OpenOpts, SessionPhase},
    runtime::{Core, CoreOptions},
    store::{PairedHostRow, StoredIdentity},
    tables::SessionTable,
};
use rsp_core::{hid::InputMsg, pb, trust::Permission};
use rsp_transport::identity::{CertKind, ExternalSigner, LocalIdentity, SignError};
use serde_json::Value;
use tokio::time::{sleep, timeout};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let root = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp")
            });
        let path = root.join(format!(
            "rs-mobile-log-markers-{}-{label}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct TestSigner {
    key: EcdsaKeyPair,
    spki: Vec<u8>,
    count: AtomicUsize,
}

impl TestSigner {
    fn new() -> Arc<Self> {
        let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let key = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_ASN1_SIGNING,
            &pair.serialize_der(),
            &SystemRandom::new(),
        )
        .unwrap();
        Arc::new(Self {
            key,
            spki: pair.subject_public_key_info(),
            count: AtomicUsize::new(0),
        })
    }
}

impl ExternalSigner for TestSigner {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.key
            .sign(&SystemRandom::new(), msg)
            .map(|signature| signature.as_ref().to_vec())
            .map_err(|error| SignError(error.to_string()))
    }

    fn spki_der(&self) -> Vec<u8> {
        self.spki.clone()
    }
}

#[derive(Default)]
struct EventCollector {
    collected: Mutex<Vec<(u64, Value)>>,
}

impl rs_mobile::uniffi_api::EventSink for EventCollector {
    fn on_event(&self, seq: u64, payload: String) {
        self.collected
            .lock()
            .unwrap()
            .push((seq, serde_json::from_str(&payload).unwrap()));
    }
}

fn new_core(root: &Path) -> Arc<Core> {
    let core = Core::new(
        CoreConfig {
            files_dir: None,
            data_dir: root.join("core").to_str().unwrap().to_owned(),
            app_version: "test".to_owned(),
            platform: DevicePlatform::Ios,
        },
        CoreOptions {
            init_logging: false,
            worker_threads: 2,
            ..CoreOptions::default()
        },
    )
    .unwrap();
    core.set_signer(Some(TestSigner::new()));
    let identity =
        LocalIdentity::generate_external(CertKind::App, core.signer().expect("test signer"))
            .unwrap();
    core.store()
        .set_identity(&StoredIdentity {
            cert_der: identity.cert_der.to_vec(),
            device_name: "Log marker device".to_owned(),
            created_at_us: core.now_us(),
            registered_at_us: None,
        })
        .unwrap();
    core.identity_changed();
    core
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

fn files_under(path: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, files);
        } else {
            files.push(path);
        }
    }
}

fn read_logs(paths: &[PathBuf]) -> (usize, String) {
    let mut files = Vec::new();
    for path in paths {
        files_under(path, &mut files);
    }
    let mut text = String::new();
    for path in &files {
        text.push_str(&std::fs::read_to_string(path).unwrap_or_default());
        text.push('\n');
    }
    (files.len(), text)
}

fn marker(prefix: &str, bytes: &[u8]) -> String {
    format!("{prefix}{}", data_encoding::HEXLOWER.encode(&bytes[..8]))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sec14_markers_never_reach_logs() {
    let root = TempDir::new("sec14");
    let logs = root.path().join("logs");
    assert!(
        rs_mobile::rscore::logging::init_global(&logs, "trace").unwrap(),
        "global logger must be initialized by this process"
    );

    let core = new_core(root.path());
    let events = Arc::new(EventCollector::default());
    core.events().subscribe(events.clone());
    let identity = core.local_identity().unwrap();
    let fingerprint = identity.fingerprint();
    let text_marker = marker("RS-SEC14-", &fingerprint[..8]);
    let clipboard_marker = marker("RS-SEC14-", &fingerprint[8..16]);

    let platform = FakePlatform::new();
    let host = TestHost::start_with_platform(
        platform.clone(),
        TestHostOptions {
            tcp: false,
            ..Default::default()
        },
    )
    .await;
    host.pair(&identity, Permission::Control);
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host.host_id.clone(),
            fp: host.fp,
            name: "Log marker host".into(),
            addresses: vec![host.quic_addr],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();

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
    until(Duration::from_secs(5), || {
        entry.summary().phase == SessionPhase::Streaming && entry.summary().display_id.is_some()
    })
    .await;

    sessions::text(&core, id, &text_marker).unwrap();
    until(Duration::from_secs(2), || {
        platform
            .injected_msgs()
            .iter()
            .any(|message| matches!(message, InputMsg::Text(text) if text == &text_marker))
    })
    .await;

    assert_eq!(
        sessions::clipboard_send(&core, id, &clipboard_marker),
        Ok(())
    );

    let data = clipboard_marker.as_bytes().to_vec();
    platform.set_clipboard(Some(ClipboardItem {
        kind: pb::clipboard_header::Kind::Text,
        sha256: host_core::session::clipboard::sha256(&data),
        data,
    }));
    sleep(Duration::from_millis(200)).await;

    sessions::close(&core, id);
    let session_id = id.to_string();
    until(Duration::from_secs(2), || {
        events.collected.lock().unwrap().iter().any(|(_, event)| {
            event["type"] == "session.state"
                && event["sessionId"] == session_id.as_str()
                && event["phase"] == "ended"
        })
    })
    .await;
    core.shutdown();
    host.abort();

    let paths = [
        root.path().to_path_buf(),
        core.log_dir(),
        host.state_dir.clone(),
    ];
    let host_port = host.quic_addr.port().to_string();
    let (mut files, mut text);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        (files, text) = read_logs(&paths);
        if files > 0 && text.contains(&host_port) && text.contains("input injection stats") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "logs did not contain a record for this Host"
        );
        sleep(Duration::from_millis(50)).await;
    }

    assert!(files > 0, "no log files were written");
    assert!(
        text.contains(&host_port),
        "logs contain no session record for this Host"
    );
    assert!(!text.contains(&text_marker), "typed text leaked into logs");
    assert!(
        !text.contains(&clipboard_marker),
        "clipboard text leaked into logs"
    );
}
