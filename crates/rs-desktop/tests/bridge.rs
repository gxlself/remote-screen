use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::EncodePublicKey,
};
use rs_desktop::bridge::{
    self, core_dir, default_platform, Bridge, CallError, DesktopConfig, EventForward, CALLS,
};
use rs_mobile::rscore::{
    dto_core::{CoreEvent, DevicePlatform},
    error::CoreError,
    runtime::{self, CoreOptions},
};
use rsp_transport::identity::{ExternalSigner, SignError};
use serde_json::{json, Value};
use tracing::{
    field::{Field, Visit},
    span, Event, Metadata, Subscriber,
};

struct TestSigner {
    key: SigningKey,
    spki: Vec<u8>,
}

impl TestSigner {
    fn new() -> Arc<TestSigner> {
        let key = loop {
            let mut scalar = [0_u8; 32];
            getrandom::fill(&mut scalar).expect("test randomness");
            if let Ok(key) = SigningKey::from_slice(&scalar) {
                break key;
            }
        };
        let spki = key
            .verifying_key()
            .to_public_key_der()
            .expect("SPKI")
            .as_bytes()
            .to_vec();
        Arc::new(TestSigner { key, spki })
    }
}

impl ExternalSigner for TestSigner {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        let signature: Signature = self.key.sign(msg);
        Ok(signature.to_der().as_bytes().to_vec())
    }

    fn spki_der(&self) -> Vec<u8> {
        self.spki.clone()
    }
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("rs-desktop-bridge-{}-{now}", std::process::id()));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Recorder {
    raw: Mutex<Vec<(u64, String)>>,
    typed: Mutex<Vec<CoreEvent>>,
}

impl Recorder {
    fn raw_events(&self) -> Vec<(u64, String)> {
        self.raw.lock().unwrap().clone()
    }

    fn typed_events(&self) -> Vec<CoreEvent> {
        self.typed.lock().unwrap().clone()
    }
}

impl EventForward for Recorder {
    fn raw(&self, seq: u64, payload: &str) {
        self.raw.lock().unwrap().push((seq, payload.to_owned()));
    }

    fn typed(&self, event: &CoreEvent) {
        self.typed.lock().unwrap().push(event.clone());
    }
}

/// 记下所有 span 与 event 的字段文本。
struct Capture(Arc<Mutex<Vec<String>>>);

struct Fields<'a>(&'a mut String);

impl Visit for Fields<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        let _ = write!(self.0, " {}={value}", field.name());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, " {}={value:?}", field.name());
    }
}

impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &span::Attributes<'_>) -> span::Id {
        let mut line = span.metadata().name().to_owned();
        span.record(&mut Fields(&mut line));
        self.0.lock().unwrap().push(line);
        span::Id::from_u64(1)
    }

    fn record(&self, _: &span::Id, values: &span::Record<'_>) {
        let mut line = String::new();
        values.record(&mut Fields(&mut line));
        self.0.lock().unwrap().push(line);
    }

    fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = event.metadata().target().to_owned();
        event.record(&mut Fields(&mut line));
        self.0.lock().unwrap().push(line);
    }

    fn enter(&self, _: &span::Id) {}

    fn exit(&self, _: &span::Id) {}
}

fn current_thread() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

#[test]
fn core_dir_joins() {
    assert_eq!(core_dir(Path::new("/a/b")), PathBuf::from("/a/b/core"));
}

#[test]
fn default_platform_linux() {
    #[cfg(not(windows))]
    assert_eq!(default_platform(), DevicePlatform::Macos);
    #[cfg(windows)]
    assert_eq!(default_platform(), DevicePlatform::Windows);
}

#[test]
fn call_error_shape() {
    let http = CallError::from(CoreError::Http {
        status: 403,
        hub_code: Some("forbidden".into()),
        message: "m".into(),
    });
    assert_eq!(
        serde_json::to_string(&http).unwrap(),
        r#"{"tag":"Http","inner":{"message":"m","status":403,"hubCode":"forbidden"}}"#
    );

    let conflict = CallError::from(CoreError::Conflict {
        message: "c".into(),
        current_json: Some(r#"{"version":3,"value":{}}"#.into()),
    });
    assert_eq!(
        serde_json::to_value(&conflict).unwrap(),
        json!({"tag": "Conflict", "inner": {"message": "c", "currentJson": r#"{"version":3,"value":{}}"#}})
    );

    let internal = CallError::from(CoreError::internal("boom"));
    assert_eq!(
        serde_json::to_string(&internal).unwrap(),
        r#"{"tag":"Internal","inner":{"message":"boom"}}"#
    );

    for (error, tag) in [
        (CoreError::network("n"), "Network"),
        (CoreError::unsupported("u"), "Unsupported"),
        (CoreError::cancelled("x"), "Cancelled"),
    ] {
        assert_eq!(CallError::from(error).tag, tag);
    }
    assert_eq!(CallError::unsupported("u").tag, "Unsupported");
    let bad = CallError::bad_args("hostsForget", "why");
    assert_eq!(bad.tag, "Internal");
    assert_eq!(bad.inner.message, "bad args for hostsForget: why");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_lifecycle() {
    let dir = TestDir::new();
    let data_dir = core_dir(dir.path());

    // a. 启动：p256 测试签名器。
    let bridge = Bridge::start(
        DesktopConfig {
            data_dir: data_dir.clone(),
            app_version: "test".into(),
            platform: DevicePlatform::Macos,
        },
        Some(TestSigner::new()),
        CoreOptions {
            init_logging: false,
            ..Default::default()
        },
    )
    .expect("bridge starts");
    assert!(runtime::core().is_ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&data_dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    // b. 身份：返回 JSON 字符串值，不解析。
    let identity = bridge
        .call("identityEnsure", json!(["测试电脑"]))
        .await
        .expect("identity");
    let identity: Value =
        serde_json::from_str(identity.as_str().expect("JSON string value")).unwrap();
    assert!(identity["deviceId"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    assert!(identity["fp"].as_str().is_some_and(|fp| !fp.is_empty()));

    // c、d.
    assert_eq!(
        bridge.call("hostsPaired", json!([])).await.unwrap(),
        json!("[]")
    );
    assert_eq!(
        bridge.call("sessionsList", json!([])).await.unwrap(),
        json!("[]")
    );

    // e. 开会话走外壳命令。
    let error = bridge
        .call("sessionsOpen", json!(["h", "{}"]))
        .await
        .unwrap_err();
    assert_eq!(error.tag, "Unsupported");
    assert_eq!(
        error.inner.message,
        "not available on desktop: sessionsOpen"
    );

    // f. 参数个数不对。
    let error = bridge.call("hostsForget", json!([])).await.unwrap_err();
    assert_eq!(error.tag, "Internal");
    assert!(error.inner.message.contains("bad args"), "{error:?}");

    // g. u64 接受十进制字符串和数字。
    assert_eq!(
        bridge
            .call("sessionsClose", json!(["18446744073709551615"]))
            .await
            .unwrap(),
        Value::Null
    );
    assert_eq!(
        bridge.call("sessionsClose", json!([5])).await.unwrap(),
        Value::Null
    );

    // h. Core 已由外壳启动。
    assert_eq!(
        bridge.call("coreInit", json!(["{}"])).await.unwrap(),
        Value::Null
    );

    // i. 事件：原样转发 + 解析后转发。
    let recorder = Arc::new(Recorder::default());
    let subscription = bridge.subscribe(recorder.clone());
    let event = CoreEvent::SessionDisplays {
        session_id: "7".into(),
        displays: vec![],
        current_id: 0,
    };
    bridge.core().events().emit(&event);
    assert!(bridge.core().events().flush(Duration::from_secs(2)));
    let raw = recorder.raw_events();
    assert_eq!(raw.len(), 1);
    assert!(raw[0].1.contains(r#""type":"session.displays""#), "{raw:?}");
    assert_eq!(recorder.typed_events(), vec![event.clone()]);

    // j. 认不出的事件只走原样转发。
    bridge
        .core()
        .events()
        .emit(&json!({"type": "future.event"}));
    bridge
        .core()
        .events()
        .emit(&json!({"type": "future.event"}));
    assert!(bridge.core().events().flush(Duration::from_secs(2)));
    let raw = recorder.raw_events();
    assert_eq!(raw.len(), 3);
    assert!(raw[1].1.contains("future.event"));
    assert!(raw[0].0 < raw[1].0 && raw[1].0 < raw[2].0);
    assert_eq!(recorder.typed_events(), vec![event.clone()]);

    // k.
    assert_eq!(
        bridge
            .call("logsWrite", json!(["info", "t", "m"]))
            .await
            .unwrap(),
        Value::Null
    );

    // 退订后不再收到。
    bridge.unsubscribe(subscription);
    bridge.core().events().emit(&event);
    assert!(bridge.core().events().flush(Duration::from_secs(2)));
    assert_eq!(recorder.raw_events().len(), 3);

    // l. 关两次不 panic，全局 core 已取下。
    bridge.shutdown();
    bridge.shutdown();
    assert!(runtime::core().is_err());
    let error = bridge.call("hostsPaired", json!([])).await.unwrap_err();
    assert_eq!(error.tag, "Internal");
}

#[test]
fn secret_args_not_logged() {
    // Account-password pairing is unsupported; a pairing file's
    // text is the remaining secret argument the bridge must not log.
    const SECRET: &str = "s3cret-口令";
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rt = current_thread();
    let src = json!({ "text": SECRET }).to_string();
    let (result, bad) = tracing::subscriber::with_default(Capture(seen.clone()), || {
        let result = rt.block_on(bridge::call("pairingLoadFile", json!([src])));
        let bad = rt.block_on(bridge::call("pairingLoadFile", json!([src, SECRET])));
        (result, bad)
    });
    let error = result.unwrap_err();
    assert!(!error.inner.message.contains(SECRET));
    let bad = bad.unwrap_err();
    assert!(bad.inner.message.contains("bad args"), "{bad:?}");
    assert!(!bad.inner.message.contains(SECRET));

    let seen = seen.lock().unwrap();
    assert!(seen.iter().all(|line| !line.contains(SECRET)), "{seen:?}");
    assert!(
        seen.iter().any(|line| line.contains("pairingLoadFile")),
        "{seen:?}"
    );
}

#[test]
fn calls_cover_bindings() {
    let bindings = include_str!("../../../app/src/native/rscore/bindings.ts");
    let declared = |name: &str| {
        [format!("{name}("), format!("{name}?(")]
            .iter()
            .any(|needle| {
                bindings.match_indices(needle.as_str()).any(|(at, _)| {
                    !bindings[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                })
            })
    };
    for &name in CALLS {
        assert!(declared(name), "{name} is not a RsMobileBindings method");
    }
    let mut unique = CALLS.to_vec();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), CALLS.len());
}

#[test]
fn calls_dispatch_or_refuse() {
    let rt = current_thread();
    // 白名单里的名字都有分支：不会落到「电脑端没有」。
    for &name in CALLS {
        if let Err(error) = rt.block_on(bridge::call(name, json!([]))) {
            assert!(
                !error.inner.message.starts_with("not available on desktop"),
                "{name}: {error:?}"
            );
        }
    }
    for name in [
        "sessionsOpen",
        "sessionsKey",
        "sessionsText",
        "sessionsClipboardSend",
        "sessionsBackground",
        "sshServers",
        "filesList",
        "sftpList",
        "transfersList",
        "tailnetStatus",
        "eventsSubscribe",
        // 不再查询远端目录。
        "identityRegister",
        "hubDirectory",
        "hubMe",
        "hubWake",
        "hubSyncPut",
        "hubApprovalsRequest",
        "crashUploadPending",
        // 不再支持电脑账户密码配对。
        "pairingStartPassword",
        "nope",
    ] {
        let error = rt.block_on(bridge::call(name, json!([]))).unwrap_err();
        assert_eq!(error.tag, "Unsupported");
        assert_eq!(
            error.inner.message,
            format!("not available on desktop: {name}")
        );
    }

    // 参数规则。
    for (name, args) in [
        ("hostsForget", json!("h")),
        ("hostsForget", json!([1])),
        ("hostsForget", json!(["a", "b"])),
        ("sessionsClose", json!(["-1"])),
        ("sessionsClose", json!(["+5"])),
        ("sessionsClose", json!(["18446744073709551616"])),
        ("sessionsClose", json!([1.5])),
        ("sessionsSelectDisplay", json!(["1", 4_294_967_296_u64])),
        ("sessionsPrewarm", json!([["a", 1]])),
        ("sessionsAction", json!(["1", "k", "yes"])),
        ("sessionsRespondControl", json!(["1", "d", null])),
        ("diagProbeAddress", json!(["a", 70_000])),
        ("logsExport", json!(["true"])),
    ] {
        let error = rt.block_on(bridge::call(name, args.clone())).unwrap_err();
        assert_eq!(error.tag, "Internal", "{name} {args}");
        assert!(
            error
                .inner
                .message
                .starts_with(&format!("bad args for {name}: ")),
            "{name} {args}: {error:?}"
        );
    }
    // 形状对的参数到达 rs-mobile（这里没有 core）。
    for (name, args) in [
        ("sessionsAction", json!(["1", "k"])),
        ("sessionsAction", json!([1, "k", null])),
        ("diagProbeAddress", json!(["a"])),
        ("diagProbeAddress", json!(["a", null])),
        ("diagProbeAddress", json!(["a", "7300"])),
        ("sessionsPrewarm", json!([["a", "b"]])),
    ] {
        let error = rt.block_on(bridge::call(name, args.clone())).unwrap_err();
        assert_eq!(error.inner.message, "core not initialized", "{name} {args}");
    }
    assert_eq!(
        rt.block_on(bridge::call("pairingCancel", json!([])))
            .unwrap(),
        Value::Null
    );
    assert_eq!(
        rt.block_on(bridge::call("coreShutdown", json!([])))
            .unwrap(),
        Value::Null
    );
}

#[test]
fn bridge_futures_are_send() {
    fn send<T: Send>(_: &T) {}
    send(&bridge::call("hostsPaired", json!([])));
    fn shared<T: Send + Sync>() {}
    shared::<Bridge>();
}
