#![cfg(feature = "desktop-it")]
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::time::Duration;

use host_core::platform::{
    CaptureConfig, ClipboardItem, EncoderConfig, Permissions, Platform, PlatformError,
};
use host_linux::platform::{LinuxOptions, LinuxPlatform};
use host_linux::sys_model::DesktopSession;
use rsp_core::pb::clipboard_header::Kind;
use rsp_core::pb::Codec;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};
use x11rb::connection::Connection;

const WAIT: Duration = Duration::from_secs(10);

fn layer(expected: &str) {
    assert_eq!(
        std::env::var("RS_IT_LAYER").as_deref(),
        Ok(expected),
        "run under crates/host-linux/tests/desktop/run.sh {expected}"
    );
}

fn state_dir(name: &str) -> PathBuf {
    let root = PathBuf::from(std::env::var_os("TMPDIR").expect("TMPDIR"));
    let path = root.join(format!("rs-it-platform-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("state directory");
    path
}

fn options(name: &str, session: DesktopSession, view_only: bool) -> LinuxOptions {
    let mut options = LinuxOptions::from_env(state_dir(name), view_only);
    options.session = Some(session);
    options.portal_timeout = WAIT;
    options
}

async fn frame(platform: &LinuxPlatform, display: u32) -> host_core::platform::CapturedFrame {
    let (tx, mut rx) = mpsc::channel(4);
    let _capture = platform
        .capture(
            display,
            CaptureConfig {
                fps: 30,
                max_width: 0,
                max_height: 0,
            },
            tx,
        )
        .expect("capture");
    timeout(WAIT, rx.recv())
        .await
        .expect("frame timeout")
        .expect("capture closed")
}

#[tokio::test(flavor = "multi_thread")]
async fn x11_display_and_frame() {
    layer("x11");
    let platform = LinuxPlatform::new(options("x11-display", DesktopSession::X11, false))
        .await
        .expect("X11 platform");
    assert_eq!(platform.displays().len(), 1);
    assert!(platform.wait_ready(WAIT).await);
    let display = platform.displays()[0].clone();
    let frame = frame(&platform, display.id).await;
    assert_eq!((frame.width, frame.height), (1280, 720));
    platform.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn x11_inject_pointer() {
    layer("x11");
    let platform = LinuxPlatform::new(options("x11-input", DesktopSession::X11, false))
        .await
        .expect("X11 platform");
    let display = platform.displays()[0].clone();
    platform.inject_input(
        &rsp_core::hid::InputMsg::MouseMove { x: 0.25, y: 0.5 },
        &display,
    );
    let (connection, screen) = x11rb::connect(None).expect("X11 query connection");
    let pointer =
        x11rb::protocol::xproto::query_pointer(&connection, connection.setup().roots[screen].root)
            .expect("query pointer")
            .reply()
            .expect("pointer reply");
    assert!((i32::from(pointer.root_x) - 320).abs() <= 1);
    assert!((i32::from(pointer.root_y) - 360).abs() <= 1);
    platform.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn x11_permissions() {
    layer("x11");
    let platform = LinuxPlatform::new(options("x11-permissions", DesktopSession::X11, false))
        .await
        .expect("X11 platform");
    assert_eq!(
        platform.permissions(),
        Permissions {
            screen_recording: true,
            accessibility: true,
        }
    );
    let view_only = LinuxPlatform::new(options("x11-view-only", DesktopSession::X11, true))
        .await
        .expect("view-only X11 platform");
    assert!(!view_only.permissions().accessibility);
    platform.shutdown().await;
    view_only.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn x11_clipboard_roundtrip() {
    layer("x11");
    let platform = LinuxPlatform::new(options("x11-clipboard", DesktopSession::X11, false))
        .await
        .expect("X11 platform");
    let data = "平台 clip".as_bytes().to_vec();
    let mut digest = Sha256::new();
    digest.update(&data);
    let item = ClipboardItem {
        kind: Kind::Text,
        data,
        sha256: digest.finalize().into(),
    };
    platform.clipboard_set(item.clone()).expect("set clipboard");
    let got = timeout(WAIT, async {
        loop {
            if let Some(value) = platform.clipboard_get() {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("clipboard timeout");
    assert_eq!(got.data, item.data);
    platform.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn x11_activity_ignores_ours() {
    layer("x11");
    let platform = LinuxPlatform::new(options("x11-activity", DesktopSession::X11, false))
        .await
        .expect("X11 platform");
    let (tx, mut rx) = mpsc::channel(8);
    platform.watch_local_activity(tx);
    let display = platform.displays()[0].clone();
    for _ in 0..5 {
        platform.inject_input(
            &rsp_core::hid::InputMsg::MouseMove { x: 0.25, y: 0.5 },
            &display,
        );
    }
    assert!(timeout(Duration::from_secs(1), rx.recv()).await.is_err());
    platform.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn x11_encoder_chain() {
    layer("x11");
    let platform = LinuxPlatform::new(options("x11-encoder", DesktopSession::X11, true))
        .await
        .expect("X11 platform");
    assert!(!platform.encoder_chain(Codec::Hevc).contains(&"openh264"));
    if std::env::var_os("RS_FFMPEG_DIR").is_some()
        && std::env::var("RS_IT_GPU").as_deref() == Ok("none")
    {
        let (sink, _output) = mpsc::channel(1);
        let result = platform.encoder(EncoderConfig {
            codec: Codec::Hevc,
            width: 640,
            height: 360,
            fps: 30,
            bitrate_kbps: 1000,
            sink,
        });
        assert!(matches!(result, Err(PlatformError::Unsupported)));
    }
    platform.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn wl_portal_grant() {
    layer("sway");
    let platform = LinuxPlatform::new(options("wl-grant", DesktopSession::Wayland, true))
        .await
        .expect("Wayland platform");
    assert!(platform.wait_ready(WAIT).await);
    let displays = platform.displays();
    assert_eq!(displays.len(), 1);
    assert_eq!(
        platform.permissions(),
        Permissions {
            screen_recording: true,
            accessibility: false,
        }
    );
    let frame = frame(&platform, displays[0].id).await;
    assert_eq!((frame.width, frame.height), (1280, 720));
    platform.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn wl_request_permission_reopens() {
    layer("sway");
    let platform = LinuxPlatform::new(options("wl-reopen", DesktopSession::Wayland, true))
        .await
        .expect("Wayland platform");
    let (tx, mut rx) = mpsc::channel(8);
    platform.watch_displays(tx);
    assert!(platform.wait_ready(WAIT).await);
    platform.request_permission(host_core::platform::PermissionKind::Accessibility);
    let deadline = Instant::now() + WAIT;
    let mut saw_empty = false;
    let mut saw_display = false;
    while Instant::now() < deadline && !(saw_empty && saw_display) {
        if let Ok(Some(value)) = timeout(Duration::from_millis(500), rx.recv()).await {
            saw_empty |= value.is_empty();
            saw_display |= !value.is_empty();
        }
    }
    assert!(saw_empty);
    assert!(saw_display);
    platform.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn wl_restore_token_file() {
    layer("sway");
    let state = state_dir("wl-token");
    let mut options = LinuxOptions::from_env(state.clone(), true);
    options.session = Some(DesktopSession::Wayland);
    options.portal_timeout = WAIT;
    let platform = LinuxPlatform::new(options).await.expect("Wayland platform");
    assert!(platform.wait_ready(WAIT).await);
    let path = state.join("portal-restore.json");
    if path.exists() {
        let metadata = std::fs::metadata(&path).expect("restore metadata");
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::MetadataExt::mode(&metadata) & 0o777,
            0o600
        );
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("restore token file"))
                .expect("restore JSON");
        assert_eq!(value["kind"], "screencast");
    }
    platform.shutdown().await;
}
