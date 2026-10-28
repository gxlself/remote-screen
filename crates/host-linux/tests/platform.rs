#![cfg(target_os = "linux")]

use std::fs;
use std::net::{IpAddr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use host_core::platform::{
    CaptureConfig, ClipboardItem, EncoderConfig, Nic, PermissionKind, Platform, PlatformError,
    SessionActionKind,
};
use host_linux::platform::{
    pick_openh264, portal_retry, portal_target, raw_to_logical, session_action_plan, ActionPlan,
    LinuxOptions, LinuxPlatform, PortalRetry,
};
use host_linux::portal::PortalError;
use host_linux::portal_model::StreamDisplay;
use host_linux::sys_model::DesktopSession;
use host_linux::wl_capture::RawPointer;
use rsp_core::pb::clipboard_header::Kind;
use rsp_core::pb::Codec;
use tokio::sync::mpsc;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"));
        let path = root.join(format!("rs-platform-test-{}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("create test state directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn displays() -> Vec<StreamDisplay> {
    vec![
        StreamDisplay {
            id: 1,
            node_id: 41,
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        },
        StreamDisplay {
            id: 2,
            node_id: 42,
            x: 1920,
            y: 0,
            width: 1280,
            height: 1024,
            primary: false,
        },
    ]
}

#[test]
fn retry_policy() {
    assert_eq!(
        portal_retry(&PortalError::Cancelled, 0),
        PortalRetry::WaitForRequest
    );
    assert_eq!(
        portal_retry(&PortalError::Timeout, 0),
        PortalRetry::After(Duration::from_secs(300))
    );
    assert_eq!(
        portal_retry(&PortalError::Unavailable, 0),
        PortalRetry::After(Duration::from_secs(10))
    );
    assert_eq!(
        portal_retry(&PortalError::Failed("x".into()), 0),
        PortalRetry::After(Duration::from_secs(1))
    );
    assert_eq!(
        portal_retry(&PortalError::Failed("x".into()), 3),
        PortalRetry::After(Duration::from_secs(8))
    );
    assert_eq!(
        portal_retry(&PortalError::Failed("x".into()), 10),
        PortalRetry::After(Duration::from_secs(60))
    );
}

#[test]
fn target_and_pointer() {
    let displays = displays();
    let target = portal_target(&displays, 2).expect("display target");
    assert_eq!(target.node_id, 42);
    assert_eq!(target.geom.width, 1280.0);
    assert_eq!(target.geom.height, 1024.0);
    assert!(portal_target(&displays, 7).is_none());
    let mut zero = displays.clone();
    zero[0].width = 0;
    assert!(portal_target(&zero, 1).is_none());
    assert_eq!(
        raw_to_logical(
            &RawPointer {
                display_id: 2,
                nx: 0.5,
                ny: 0.25,
                visible: true,
            },
            &displays,
        ),
        Some((2, 640.0, 256.0))
    );
}

#[test]
fn action_plan() {
    for (logind, injector, expected_lock, expected_wake) in [
        (
            false,
            false,
            ActionPlan::Unsupported,
            ActionPlan::Unsupported,
        ),
        (true, false, ActionPlan::Lock, ActionPlan::Unsupported),
        (false, true, ActionPlan::Unsupported, ActionPlan::Wake),
        (true, true, ActionPlan::Lock, ActionPlan::Wake),
    ] {
        assert_eq!(
            session_action_plan(SessionActionKind::LockScreen, logind, injector),
            expected_lock
        );
        assert_eq!(
            session_action_plan(SessionActionKind::WakeDisplay, logind, injector),
            expected_wake
        );
        assert_eq!(
            session_action_plan(SessionActionKind::LockOnEnd, logind, injector),
            ActionPlan::Nothing
        );
        assert_eq!(
            session_action_plan(SessionActionKind::CtrlAltDel, logind, injector),
            ActionPlan::Unsupported
        );
        assert_eq!(
            session_action_plan(SessionActionKind::Unspecified, logind, injector),
            ActionPlan::Unsupported
        );
    }
}

#[test]
fn pick_blob() {
    let candidates = vec![PathBuf::from("one"), PathBuf::from("two")];
    assert_eq!(
        pick_openh264(&candidates, |path| path == Path::new("two")),
        Some(candidates[1].clone())
    );
    assert_eq!(
        pick_openh264(&candidates, |_| false),
        Some(candidates[0].clone())
    );
    assert_eq!(pick_openh264(&[], |_| true), None);
}

#[tokio::test]
async fn none_backend() {
    let temp = TempDir::new();
    let mut options = LinuxOptions::from_env(temp.path().to_owned(), false);
    options.session = Some(DesktopSession::None);
    let platform = LinuxPlatform::new(options)
        .await
        .expect("headless platform");
    assert!(platform.displays().is_empty());
    let (frames, _rx) = mpsc::channel(1);
    assert!(matches!(
        platform.capture(
            1,
            CaptureConfig {
                fps: 1,
                max_width: 0,
                max_height: 0,
            },
            frames,
        ),
        Err(PlatformError::Unsupported)
    ));
    assert!(!platform.permissions().screen_recording);
    assert!(!platform.permissions().accessibility);
    assert!(platform.clipboard_get().is_none());
    let item = ClipboardItem {
        kind: Kind::Text,
        data: b"text".to_vec(),
        sha256: [0; 32],
    };
    assert!(matches!(
        platform.clipboard_set(item),
        Err(PlatformError::Unsupported)
    ));
    assert!(matches!(
        platform.session_action(SessionActionKind::CtrlAltDel),
        Err(PlatformError::Unsupported)
    ));
    let info = platform.info();
    assert_eq!(info.os, rsp_core::pb::Platform::Linux);
    assert!(!info.name.is_empty());
    let _ = platform.nics();
    let _ = platform.console_state();
    assert!(!platform.wait_ready(Duration::from_millis(200)).await);
    platform.shutdown().await;
}

#[tokio::test]
async fn keystore_files() {
    let temp = TempDir::new();
    let mut options = LinuxOptions::from_env(temp.path().to_owned(), false);
    options.session = Some(DesktopSession::None);
    let platform = LinuxPlatform::new(options)
        .await
        .expect("headless platform");
    platform
        .keystore()
        .store("host_key", b"x")
        .expect("store key");
    let path = temp.path().join("keys/host_key");
    assert!(path.exists());
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::MetadataExt::mode(&fs::metadata(&path).expect("key metadata")) & 0o777,
        0o600
    );
    assert_eq!(
        platform.keystore().load("host_key").expect("load key"),
        Some(b"x".to_vec())
    );
    platform.keystore().delete("host_key").expect("delete key");
    assert_eq!(
        platform
            .keystore()
            .load("host_key")
            .expect("load deleted key"),
        None
    );
    platform.shutdown().await;
}

#[tokio::test]
async fn encoder_unavailable() {
    let temp = TempDir::new();
    let mut options = LinuxOptions::from_env(temp.path().to_owned(), true);
    options.session = Some(DesktopSession::None);
    options.encoder_pref = Some(host_linux::ff_model::LinuxEncoderPref::OpenH264);
    options.openh264_path = Some("/nonexistent/libopenh264.so.7".into());
    let platform = LinuxPlatform::new(options)
        .await
        .expect("headless platform");
    assert_eq!(
        platform.encoder_chain(Codec::H264).first().copied(),
        Some("openh264")
    );
    let (sink, _rx) = mpsc::channel(1);
    let result = platform.encoder(EncoderConfig {
        codec: Codec::H264,
        width: 640,
        height: 360,
        fps: 30,
        bitrate_kbps: 1000,
        sink,
    });
    assert!(result.is_err());
    platform.shutdown().await;
}

#[test]
fn wol_needs_ipv4() {
    let temp = TempDir::new();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let platform = runtime
        .block_on(async {
            let mut options = LinuxOptions::from_env(temp.path().to_owned(), true);
            options.session = Some(DesktopSession::None);
            LinuxPlatform::new(options).await
        })
        .expect("headless platform");
    let nic = Nic {
        name: "eth0".into(),
        addresses: vec![IpAddr::V6(Ipv6Addr::LOCALHOST)],
        networks: Vec::new(),
        mac: Some([0; 6]),
        tailnet: false,
    };
    assert!(platform.send_wol([0; 6], &nic).is_err());
    runtime.block_on(platform.shutdown());
}

#[allow(dead_code)]
fn _permission_kind_is_public(_: PermissionKind) {}
