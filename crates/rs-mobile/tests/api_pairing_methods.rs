mod support;

use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use data_encoding::BASE64URL_NOPAD;
use host_core::pairing::ExportFileRequest;
use host_fake::harness::TestHost;
use rs_mobile::rscore::api_pairing::{
    load_file, paired, resolve_target, result_outcome, start, LoadFileSrc, PairFileCache,
};
use rs_mobile::rscore::dto_core::{PairFileSummary, PairOutcome, PairStartArg, PairTarget};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::Core;
use rsp_core::ids::{fp_b64url, id_from_fp};
use rsp_core::pairfile::{self, PairFile, PAIRFILE_MAX_BYTES};
use rsp_core::pb;
use rsp_core::trust::Permission;
use serde_json::json;
use support::{test_core_with, with_identity, EventCollector};
use tokio::time::timeout;
use zeroize::Zeroizing;

const NOW: u64 = 1_792_465_134_000_000;
const ACCOUNT: &str = "mobile-pair-private-account";
const PASSWORD: &str = "mobile-pair-private-password";
const WRONG_PASSWORD: &str = "mobile-pair-private-wrong-password";
const SECRET: [u8; 32] = *b"mobile-file-private-secret-1234!";

fn configured_core(clock: Arc<AtomicU64>) -> Arc<Core> {
    logs();
    let core = test_core_with(|_, opts| {
        opts.clock = Some(Arc::new(move || clock.load(Ordering::Relaxed)));
        opts.timeouts.pairing = Duration::from_secs(6);
    });
    with_identity(&core, "Pair methods phone");
    core
}

fn file(id: u8) -> PairFile {
    let fp = [7; 32];
    PairFile {
        v: 1,
        host_id: id_from_fp(&fp),
        fp,
        file_id: [id; 16],
        secret: Zeroizing::new(SECRET),
        name: "Example Host".into(),
        addrs: vec!["100.64.0.7:9870".parse().unwrap()],
        expires_at_s: NOW / 1_000_000 + 86_400,
    }
}

fn text_src(text: String) -> LoadFileSrc {
    LoadFileSrc::Text {
        text: Zeroizing::new(text),
    }
}

fn file_arg(summary: &PairFileSummary) -> PairStartArg {
    serde_json::from_value(json!({"fileId":summary.file_id})).unwrap()
}

fn assert_error(outcome: PairOutcome, message: &str) {
    assert_eq!(
        outcome,
        PairOutcome::Error {
            code: None,
            message: message.into()
        }
    );
}

fn collect(core: &Arc<Core>) -> Arc<EventCollector> {
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    events
}

fn assert_done(core: &Arc<Core>, events: &EventCollector, outcome: &PairOutcome) {
    assert!(core.events().flush(Duration::from_secs(1)));
    let done = events.of_type("pairing.done");
    assert_eq!(done.len(), 1);
    assert_eq!(done[0]["outcome"], serde_json::to_value(outcome).unwrap());
    assert_eq!(events.events().last().unwrap().1["type"], "pairing.done");
}

#[test]
fn dto_shapes_preserve_existing_arguments_and_new_statuses() {
    for (outcome, value) in [
        (
            PairOutcome::Locked {
                retry_after_s: Some(873),
            },
            json!({"status":"locked", "retryAfterS":873}),
        ),
        (
            PairOutcome::Locked {
                retry_after_s: None,
            },
            json!({"status":"locked"}),
        ),
        (
            PairOutcome::MethodDisabled,
            json!({"status":"methodDisabled"}),
        ),
    ] {
        assert_eq!(serde_json::to_value(&outcome).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<PairOutcome>(value).unwrap(),
            outcome
        );
    }
    for retry in [0, 873] {
        assert_eq!(
            result_outcome(
                pb::PairResult {
                    status: pb::pair_result::Status::Locked as i32,
                    retry_after_s: retry,
                    ..Default::default()
                },
                "host",
                None,
                0
            ),
            Some(PairOutcome::Locked {
                retry_after_s: (retry > 0).then_some(retry)
            })
        );
    }
    let arg = PairStartArg::File {
        file_id: "x".into(),
    };
    assert_eq!(
        serde_json::from_value::<PairStartArg>(json!({"fileId":"x"})).unwrap(),
        arg
    );
    assert_eq!(serde_json::to_value(arg).unwrap(), json!({"fileId":"x"}));
    // Pairing requires an explicit address rather than a directory lookup.
    assert!(
        serde_json::from_value::<PairStartArg>(json!({"hostId":"host", "code":"123456"})).is_err()
    );
    let target = PairTarget {
        v: 1,
        host_id: id_from_fp(&[7; 32]),
        fp: fp_b64url(&[7; 32]),
        code: "123456".into(),
        name: "Host".into(),
        addrs: vec!["100.64.0.7:9870".into()],
    };
    let mut value = serde_json::to_value(&target).unwrap();
    value["fileId"] = json!("ignored");
    assert_eq!(
        serde_json::from_value::<PairStartArg>(value).unwrap(),
        PairStartArg::Target(target)
    );
    assert!(serde_json::from_value::<PairStartArg>(json!({"v":null,"fileId":"x"})).is_err());
    assert!(serde_json::from_value::<PairStartArg>(
        json!({"fileId":7,"hostId":"host","code":"123456"})
    )
    .is_err());
    assert!(serde_json::from_value::<PairStartArg>(json!({"hostId":"host"})).is_err());
}

#[test]
fn load_text_and_file_return_only_public_summary_and_redact_debug() {
    let core = configured_core(Arc::new(AtomicU64::new(NOW)));
    let file = file(1);
    let src: LoadFileSrc =
        serde_json::from_value(json!({"text":format!(" \r\n{}\n\t",file.to_uri())})).unwrap();
    let debug = format!("{src:?}");
    let summary = load_file(&core, src).unwrap();
    let value = serde_json::to_value(&summary).unwrap();
    assert_eq!(
        value,
        json!({"fileId":BASE64URL_NOPAD.encode(&file.file_id), "hostId":file.host_id, "hostName":file.name, "fp":fp_b64url(&file.fp), "addrs":["100.64.0.7:9870"], "expiresAt":file.expires_at_s*1_000_000, "expired":false})
    );
    let debug = format!("{debug} {file:?} {:?}", core.state::<PairFileCache>());
    for secret in [
        String::from_utf8(SECRET.to_vec()).unwrap(),
        BASE64URL_NOPAD.encode(&SECRET),
    ] {
        assert!(!value.to_string().contains(&secret));
        assert!(!debug.contains(&secret));
    }
    assert!(!value.to_string().contains("secret"));
    assert!(debug.contains("<redacted>"));
    let path = core.data_dir().join("pair file.rspair");
    std::fs::write(&path, file.to_uri()).unwrap();
    let src =
        serde_json::from_value(json!({"uri":url::Url::from_file_path(path).unwrap().as_str()}))
            .unwrap();
    assert_eq!(load_file(&core, src).unwrap(), summary);
}

#[test]
fn invalid_sources_have_fixed_errors_and_expired_files_still_load() {
    let core = configured_core(Arc::new(AtomicU64::new(NOW)));
    assert!(matches!(
        load_file(
            &core,
            LoadFileSrc::Uri {
                uri: "content://x".into()
            }
        ),
        Err(CoreError::Unsupported { .. })
    ));
    let path = core.data_dir().join("invalid.rspair");
    for contents in [vec![b'x'; PAIRFILE_MAX_BYTES + 1], vec![0xff, 0xfe]] {
        std::fs::write(&path, contents).unwrap();
        assert_eq!(
            load_file(
                &core,
                LoadFileSrc::Uri {
                    uri: url::Url::from_file_path(&path).unwrap().to_string()
                }
            )
            .unwrap_err(),
            CoreError::internal("not a pairing file")
        );
    }
    for text in [
        "garbage".to_owned(),
        "rsp://pair?v=1".to_owned(),
        "x".repeat(PAIRFILE_MAX_BYTES + 1),
    ] {
        assert_eq!(
            load_file(&core, text_src(text)).unwrap_err(),
            CoreError::internal("not a pairing file")
        );
    }
    let mut file = file(2);
    file.expires_at_s = NOW / 1_000_000;
    assert!(load_file(&core, text_src(file.to_uri())).unwrap().expired);
}

#[tokio::test]
async fn cache_expires_at_ten_minutes_and_expired_files_never_dial() {
    let now = Arc::new(AtomicU64::new(NOW));
    let core = configured_core(now.clone());
    let summary = load_file(&core, text_src(file(1).to_uri())).unwrap();
    now.store(NOW + 600_000_000 - 1, Ordering::Relaxed);
    assert!(resolve_target(&core, file_arg(&summary)).await.is_ok());
    now.store(NOW + 600_000_000, Ordering::Relaxed);
    assert_error(
        start(&core, file_arg(&summary)).await.unwrap(),
        "pairing file not loaded",
    );
    let mut file = file(2);
    file.expires_at_s = core.now_us() / 1_000_000;
    let summary = load_file(&core, text_src(file.to_uri())).unwrap();
    let events = collect(&core);
    let outcome = start(&core, file_arg(&summary)).await.unwrap();
    assert_eq!(outcome, PairOutcome::Expired);
    assert_done(&core, &events, &outcome);
    assert!(events.of_type("pairing.progress").is_empty());
}

#[tokio::test]
async fn cache_evicts_oldest_and_reloading_replaces_and_renews_entry() {
    let now = Arc::new(AtomicU64::new(NOW));
    let core = configured_core(now.clone());
    let mut summaries = Vec::new();
    for id in 0..9 {
        summaries.push(load_file(&core, text_src(file(id).to_uri())).unwrap());
    }
    assert_error(
        start(&core, file_arg(&summaries[0])).await.unwrap(),
        "pairing file not loaded",
    );
    for summary in &summaries[1..] {
        assert!(resolve_target(&core, file_arg(summary)).await.is_ok());
    }
    now.store(NOW + 599_000_000, Ordering::Relaxed);
    let mut changed = file(1);
    changed.name = "Reloaded Host".into();
    changed.secret[0] ^= 1;
    let summary = load_file(&core, text_src(changed.to_uri())).unwrap();
    now.store(NOW + 600_000_000, Ordering::Relaxed);
    let (target, credential) = resolve_target(&core, file_arg(&summary)).await.unwrap();
    assert_eq!(target.host_name, changed.name);
    let rsp_client::session::PairingCredential::File { secret, .. } = credential else {
        panic!("file credential")
    };
    assert_eq!(secret, changed.secret);
    assert_error(
        start(&core, file_arg(&summaries[2])).await.unwrap(),
        "pairing file not loaded",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_file_pairing_persists_consumes_and_reports_revocation() {
    timeout(Duration::from_secs(12), async {
        let now = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_micros(),
        )
        .unwrap();
        let host = TestHost::start().await;
        let core = configured_core(Arc::new(AtomicU64::new(now)));
        let exported = host
            .security
            .pairing
            .export_file(
                now,
                ExportFileRequest {
                    permission: Permission::Control,
                    label: None,
                    dir: core.data_dir(),
                },
            )
            .unwrap();
        let raw = Zeroizing::new(std::fs::read_to_string(&exported.path).unwrap());
        let file = pairfile::parse(&raw).unwrap();
        let summary = load_file(
            &core,
            LoadFileSrc::Uri {
                uri: url::Url::from_file_path(&exported.path)
                    .unwrap()
                    .to_string(),
            },
        )
        .unwrap();
        let (target, _) = resolve_target(&core, file_arg(&summary)).await.unwrap();
        assert_eq!(target.fp, file.fp);
        assert_eq!(target.host_name, file.name);
        assert_eq!(
            target.candidates.iter().map(|c| c.addr).collect::<Vec<_>>(),
            vec![host.quic_addr]
        );
        let events = collect(&core);
        let outcome = start(&core, file_arg(&summary)).await.unwrap();
        assert!(matches!(outcome, PairOutcome::Ok { .. }), "{outcome:?}");
        assert_done(&core, &events, &outcome);
        let hosts = paired(&core).unwrap();
        assert_eq!(hosts[0].host_id, host.host_id);
        assert_eq!(hosts[0].fp, summary.fp);
        assert_error(
            start(&core, file_arg(&summary)).await.unwrap(),
            "pairing file not loaded",
        );
        let exported = host
            .security
            .pairing
            .export_file(
                now,
                ExportFileRequest {
                    permission: Permission::Control,
                    label: None,
                    dir: core.data_dir(),
                },
            )
            .unwrap();
        let raw = Zeroizing::new(std::fs::read_to_string(exported.path).unwrap());
        let revoked = pairfile::parse(&raw).unwrap();
        host.security
            .pairing
            .revoke_file(&exported.file_id)
            .unwrap();
        let summary = load_file(&core, text_src(revoked.to_uri())).unwrap();
        for _ in 0..2 {
            assert_eq!(
                start(&core, file_arg(&summary)).await.unwrap(),
                PairOutcome::Expired
            );
        }
        host.stop().await;
        assert_logs_redacted(&[
            BASE64URL_NOPAD.encode(file.secret.as_ref()),
            BASE64URL_NOPAD.encode(revoked.secret.as_ref()),
        ]);
    })
    .await
    .unwrap();
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);
impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn logs() -> &'static Log {
    static LOG: OnceLock<Log> = OnceLock::new();
    LOG.get_or_init(|| {
        let log = Log::default();
        let writer = log.clone();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_max_level(tracing::Level::DEBUG)
                .with_writer(move || writer.clone())
                .finish(),
        )
        .unwrap();
        tracing::debug!("mobile pairing capture active");
        log
    })
}
fn assert_logs_redacted(file_secrets: &[String]) {
    let captured = String::from_utf8(logs().0.lock().unwrap().clone()).unwrap();
    assert!(captured.contains("mobile pairing capture active"));
    for secret in [
        ACCOUNT,
        PASSWORD,
        WRONG_PASSWORD,
        std::str::from_utf8(&SECRET).unwrap(),
    ]
    .into_iter()
    .chain(file_secrets.iter().map(String::as_str))
    {
        assert!(!captured.contains(secret), "sensitive data in logs");
    }
}
