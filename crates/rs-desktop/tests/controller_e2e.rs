use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use host_fake::{
    clip::ClipPlatform,
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::EncodePublicKey,
};
use rs_desktop::{
    bridge::{Bridge, DesktopConfig},
    controller::{ControllerObserver, ControllerSettings, DesktopSessions, OpenRequest},
    platform::{
        BitstreamFormat, DecodeResult, DesktopFrame, FrameDone, FrameSink, HostOs, KeyInput,
        PlatformError, PointerInput, SessionMenu, SessionWindow, SessionWindowFactory, StreamInfo,
        VideoCodec, WindowChange, WindowCommand, WindowEvents, WindowSpec,
    },
};
use rs_mobile::rscore::{
    dto_core::{Codec, DevicePlatform},
    runtime::{Core, CoreOptions},
    store::PairedHostRow,
    tables::SessionTable,
};
use rsp_core::{
    hid::{Button, InputMsg},
    trust::Permission,
};
use rsp_transport::identity::{ExternalSigner, SignError};
use serde_json::json;
use tokio::time::{sleep, timeout};

const TWO_SECONDS: Duration = Duration::from_secs(2);
const FIVE_SECONDS: Duration = Duration::from_secs(5);
const HOST_NAME: &str = "书房 Mac";

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
        let path = std::env::temp_dir().join(format!(
            "rs-desktop-controller-{}-{now}",
            std::process::id()
        ));
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
struct Sink {
    configs: Mutex<Vec<StreamInfo>>,
    submits: AtomicUsize,
    closes: AtomicUsize,
}

impl FrameSink for Sink {
    fn format(&self) -> BitstreamFormat {
        BitstreamFormat::AnnexB
    }

    fn configure(&self, info: &StreamInfo) {
        self.configs.lock().unwrap().push(info.clone());
    }

    fn submit(&self, _frame: DesktopFrame, done: FrameDone) {
        self.submits.fetch_add(1, Ordering::SeqCst);
        done.finish(DecodeResult::Shown { decode_us: 1000 });
    }

    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Window {
    sink: Arc<Sink>,
    commands: Mutex<Vec<WindowCommand>>,
}

impl Window {
    fn has(&self, command: &WindowCommand) -> bool {
        self.commands.lock().unwrap().contains(command)
    }
}

impl SessionWindow for Window {
    fn frame_sink(&self) -> Arc<dyn FrameSink> {
        self.sink.clone()
    }

    fn apply(&self, cmd: WindowCommand) {
        self.commands.lock().unwrap().push(cmd);
    }
}

struct Created {
    spec: WindowSpec,
    events: Arc<dyn WindowEvents>,
    window: Arc<Window>,
}

struct Factory {
    hevc: bool,
    h264: bool,
    created: Mutex<Vec<Arc<Created>>>,
}

impl Factory {
    fn new(hevc: bool, h264: bool) -> Arc<Self> {
        Arc::new(Self {
            hevc,
            h264,
            created: Mutex::new(Vec::new()),
        })
    }

    fn count(&self) -> usize {
        self.created.lock().unwrap().len()
    }

    fn get(&self, index: usize) -> Arc<Created> {
        self.created.lock().unwrap()[index].clone()
    }
}

impl SessionWindowFactory for Factory {
    fn create(
        &self,
        spec: &WindowSpec,
        events: Arc<dyn WindowEvents>,
    ) -> Result<Arc<dyn SessionWindow>, PlatformError> {
        let window = Arc::new(Window::default());
        self.created.lock().unwrap().push(Arc::new(Created {
            spec: spec.clone(),
            events,
            window: window.clone(),
        }));
        Ok(window)
    }

    fn can_decode(&self, codec: VideoCodec) -> bool {
        match codec {
            VideoCodec::Hevc => self.hevc,
            VideoCodec::H264 => self.h264,
        }
    }

    fn screen_pixel_width(&self) -> u32 {
        2560
    }
}

#[derive(Default)]
struct Observer {
    ended: Mutex<Vec<(u64, String, bool)>>,
}

impl Observer {
    fn endings(&self) -> Vec<(u64, String, bool)> {
        self.ended.lock().unwrap().clone()
    }
}

impl ControllerObserver for Observer {
    fn menu_changed(&self, _session_id: u64, _menu: &SessionMenu) {}

    fn focus_changed(&self, _session_id: u64, _focused: bool) {}

    fn ended(&self, session_id: u64, host_id: &str, by_user: bool) {
        self.ended
            .lock()
            .unwrap()
            .push((session_id, host_id.to_owned(), by_user));
    }
}

/// 条件在期限内成立返回 true。
async fn until(limit: Duration, predicate: impl Fn() -> bool) -> bool {
    timeout(limit, async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}

/// `open` 会阻塞（建窗口、等视频接上），放到阻塞线程上调。
async fn open(
    sessions: &Arc<DesktopSessions>,
    req: OpenRequest,
) -> Result<String, rs_desktop::bridge::CallError> {
    let sessions = sessions.clone();
    tokio::task::spawn_blocking(move || sessions.open(req))
        .await
        .expect("open task")
}

async fn start_host(platform: impl host_core::platform::Platform + 'static) -> TestHost {
    TestHost::start_with_platform(
        platform,
        TestHostOptions {
            tcp: false,
            ..Default::default()
        },
    )
    .await
}

fn pair(core: &Core, host: &TestHost) {
    host.pair(&core.local_identity().unwrap(), Permission::Control);
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host.host_id.clone(),
            fp: host.fp,
            name: HOST_NAME.into(),
            addresses: vec![host.quic_addr],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();
}

fn open_request(host: &TestHost) -> OpenRequest {
    OpenRequest {
        host_id: host.host_id.clone(),
        host_name: HOST_NAME.into(),
        host: HostOs::Mac,
        control: true,
        display_id: None,
        quality: None,
    }
}

/// 用户关窗：会话表清空、外壳收到 by_user 结束、窗口收到 Close。
async fn close_by_user(
    core: &Core,
    sessions: &DesktopSessions,
    observer: &Observer,
    created: &Created,
    host_id: &str,
) {
    let id = created.spec.session_id;
    created.events.window(WindowChange::CloseRequested);
    assert!(
        until(TWO_SECONDS, || core.state::<SessionTable>().is_empty()).await,
        "timed out waiting for session table empty"
    );
    assert!(
        until(TWO_SECONDS, || {
            observer.endings().contains(&(id, host_id.to_owned(), true))
        })
        .await,
        "timed out waiting for ended by user"
    );
    assert!(created.window.has(&WindowCommand::Close));
    assert!(!sessions.sessions().contains(&id));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_sessions_end_to_end() {
    // 1. 准备。
    let dir = TestDir::new();
    let bridge = Bridge::start(
        DesktopConfig {
            data_dir: dir.path().join("core"),
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
    bridge
        .call("identityEnsure", json!(["e2e 电脑"]))
        .await
        .expect("identity");
    let core = bridge.core();
    // FakePlatform 默认编码器不带参数集，帧过不了闸门；内置片段有真的 HEVC VPS/SPS/PPS，
    // 输入日志仍记在 FakePlatform 上。
    let video = ClipPlatform::new();
    let platform = video.inner().clone();
    let host = start_host(video).await;
    pair(&core, &host);

    // 2. 开会话。
    let factory = Factory::new(true, true);
    let observer = Arc::new(Observer::default());
    let sessions = DesktopSessions::new(
        bridge.clone(),
        factory.clone(),
        ControllerSettings::default(),
        observer.clone(),
    );
    let request = open_request(&host);
    let id = open(&sessions, request.clone()).await.expect("open");
    let session_id: u64 = id.parse().expect("decimal session id");
    assert_eq!(factory.count(), 1);
    let first = factory.get(0);
    assert_eq!(
        first.spec,
        WindowSpec {
            session_id,
            host_id: host.host_id.clone(),
            title: HOST_NAME.into(),
            width: 1280,
            height: 800,
        }
    );
    assert_eq!(sessions.sessions(), [session_id]);

    // 3. Streaming 控制者、有画面、OpenOpts 带宽度和编码。
    let window = first.window.clone();
    assert!(
        until(FIVE_SECONDS, || {
            window.has(&WindowCommand::SetTitle(HOST_NAME.into()))
                && window.sink.submits.load(Ordering::SeqCst) > 0
        })
        .await,
        "timed out waiting for streaming title and frames: commands {:?}, submits {}, configs {:?}, sessions {:?}",
        window.commands.lock().unwrap(),
        window.sink.submits.load(Ordering::SeqCst),
        window.sink.configs.lock().unwrap(),
        core.state::<SessionTable>().summaries()
    );
    let configs = window.sink.configs.lock().unwrap().clone();
    assert!(!configs.is_empty());
    assert!(configs.iter().all(|info| info.width <= 2560), "{configs:?}");
    let entry = core
        .state::<SessionTable>()
        .get(session_id)
        .expect("session entry");
    assert_eq!(entry.opts.max_width, Some(2560));
    assert_eq!(entry.opts.codec, Some(Codec::Hevc));
    drop(entry);

    // 4. 窗口拿到焦点后按键注入到 Host。
    let events = first.events.clone();
    events.window(WindowChange::Resized {
        width: 1280.0,
        height: 800.0,
        scale: 2.0,
    });
    events.window(WindowChange::Focus(true));
    assert!(
        until(TWO_SECONDS, || {
            window.has(&WindowCommand::SetKeyboardCapture(true))
        })
        .await,
        "timed out waiting for keyboard capture"
    );
    events.key(KeyInput::MacKey {
        vk: 0x00,
        down: true,
        repeat: false,
    });
    events.key(KeyInput::MacKey {
        vk: 0x00,
        down: false,
        repeat: false,
    });
    assert!(
        until(TWO_SECONDS, || {
            let msgs = platform.injected_msgs();
            let has = |want: bool| {
                msgs.iter().any(
                    |msg| matches!(msg, InputMsg::Key { usage: 0x04, down, .. } if *down == want),
                )
            };
            has(true) && has(false)
        })
        .await,
        "timed out waiting for key injected"
    );

    // 5. 内容区中心点击。
    for down in [true, false] {
        events.pointer(PointerInput::Button {
            button: 0,
            down,
            x: 640.0,
            y: 400.0,
            clicks: 0,
        });
    }
    assert!(
        until(TWO_SECONDS, || {
            platform.injected_msgs().iter().any(|msg| {
                matches!(
                    msg,
                    InputMsg::MouseButton { button: Button::Left, down: true, x, y, .. }
                        if (x - 0.5).abs() < 0.01 && (y - 0.5).abs() < 0.01
                )
            })
        })
        .await,
        "timed out waiting for click injected"
    );

    // 6. 同一 Host 再开：同一个会话，不建新窗口。
    assert_eq!(open(&sessions, request.clone()).await.unwrap(), id);
    assert_eq!(factory.count(), 1);
    assert!(window.has(&WindowCommand::Focus));

    // 7. 用户关窗。
    close_by_user(&core, &sessions, &observer, &first, &host.host_id).await;
    assert_eq!(window.sink.closes.load(Ordering::SeqCst), 1);

    // 8. 再开：新会话、新窗口。
    let second = open(&sessions, request.clone()).await.expect("reopen");
    assert_ne!(second, id);
    assert_eq!(factory.count(), 2);
    let created = factory.get(1);
    assert_eq!(created.spec.session_id.to_string(), second);
    close_by_user(&core, &sessions, &observer, &created, &host.host_id).await;

    // 9. 编码选择。片段只有 HEVC，H.264 会话连一台 FakePlatform Host（它两种都能编）。
    let h264_host = start_host(FakePlatform::new()).await;
    pair(&core, &h264_host);
    let h264_factory = Factory::new(false, true);
    let h264 = DesktopSessions::new(
        bridge.clone(),
        h264_factory.clone(),
        ControllerSettings::default(),
        observer.clone(),
    );
    let third: u64 = open(&h264, open_request(&h264_host))
        .await
        .expect("open with H.264")
        .parse()
        .unwrap();
    let entry = core
        .state::<SessionTable>()
        .get(third)
        .expect("H.264 session entry");
    assert_eq!(entry.opts.codec, Some(Codec::H264));
    drop(entry);
    close_by_user(
        &core,
        &h264,
        &observer,
        &h264_factory.get(0),
        &h264_host.host_id,
    )
    .await;

    let none = DesktopSessions::new(
        bridge.clone(),
        Factory::new(false, false),
        ControllerSettings::default(),
        observer.clone(),
    );
    let error = open(&none, request).await.unwrap_err();
    assert_eq!(error.tag, "Unsupported");
    assert!(core.state::<SessionTable>().is_empty());
    assert!(none.sessions().is_empty());

    // 10.
    drop((sessions, h264, none));
    bridge.shutdown();
}
