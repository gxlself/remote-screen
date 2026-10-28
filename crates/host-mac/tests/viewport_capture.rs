#![cfg(target_os = "macos")]

use std::env;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use host_core::pipeline::capture::dimensions;
use host_core::platform::{
    CaptureConfig, CaptureHandle, CapturedFrame, DisplayInfo, PixelRect, Platform, PlatformError,
};
use host_mac::MacPlatform;
use tokio::sync::mpsc;

fn require_sck() -> bool {
    if env::var_os("RS_REQUIRE_SCK").is_some_and(|value| value == "1") {
        true
    } else {
        eprintln!("SKIP sck-viewport: set RS_REQUIRE_SCK=1 to require ScreenCaptureKit");
        false
    }
}

fn primary_display(platform: &MacPlatform) -> DisplayInfo {
    platform
        .displays()
        .into_iter()
        .find(|info| info.primary)
        .expect("a primary display is required")
}

// A changing window in the primary display's center prevents idle-only SCK output.
struct ScreenActivity(Child);

impl ScreenActivity {
    fn start() -> Self {
        let script = r#"
ObjC.import('AppKit');
const app = $.NSApplication.sharedApplication;
app.setActivationPolicy($.NSApplicationActivationPolicyAccessory);
const frame = $.NSScreen.screens.objectAtIndex(0).frame;
const side = Math.min(frame.size.width, frame.size.height) / 8;
const bounds = $.NSMakeRect(
    frame.origin.x + (frame.size.width - side) / 2,
    frame.origin.y + (frame.size.height - side) / 2, side, side);
const window = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer(
    bounds, $.NSWindowStyleMaskBorderless, $.NSBackingStoreBuffered, false);
window.releasedWhenClosed = false;
window.hasShadow = false;
window.level = $.NSFloatingWindowLevel;
window.collectionBehavior = $.NSWindowCollectionBehaviorCanJoinAllSpaces |
    $.NSWindowCollectionBehaviorFullScreenAuxiliary;
window.orderFrontRegardless;
for (let tick = 0; tick < 300; tick++) {
    window.backgroundColor = tick % 2 === 0 ? $.NSColor.whiteColor : $.NSColor.blackColor;
    window.displayIfNeeded;
    $.NSRunLoop.currentRunLoop.runUntilDate($.NSDate.dateWithTimeIntervalSinceNow(0.1));
}
window.close;
"#;
        Self(
            Command::new("/usr/bin/osascript")
                .args(["-l", "JavaScript", "-e", script])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .expect("start the test's changing window"),
        )
    }
}

impl Drop for ScreenActivity {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn assert_frame(frame: &CapturedFrame, display: u32, size: (u32, u32)) {
    assert_eq!(frame.display_id, display);
    assert_eq!((frame.width, frame.height), size);
}

async fn receive_frames(
    rx: &mut mpsc::Receiver<CapturedFrame>,
    display: u32,
    size: (u32, u32),
    count: usize,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    for index in 0..count {
        let frame = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("only received {index}/{count} frames within three seconds"))
            .expect("capture sink closed before receiving frames");
        assert_frame(&frame, display, size);
    }
}

async fn stop_capture(
    handle: Box<dyn CaptureHandle>,
    rx: &mut mpsc::Receiver<CapturedFrame>,
    display: u32,
    size: (u32, u32),
) {
    let stopping = Instant::now();
    drop(handle);
    // Frames queued before stop are valid; the native callbacks must be drained.
    while let Ok(frame) = rx.try_recv() {
        assert_frame(&frame, display, size);
    }
    let remaining = Duration::from_secs(1)
        .checked_sub(stopping.elapsed())
        .expect("dropping the capture handle took more than one second");
    assert!(
        tokio::time::timeout(remaining, rx.recv())
            .await
            .expect("capture sink did not close within one second")
            .is_none(),
        "received a new frame after capture stopped"
    );
}

#[test]
fn region_supported() {
    assert!(MacPlatform::new().capture_region_supported());
}

#[test]
fn region_rejects_bad_input() {
    let platform = MacPlatform::new();
    let displays = platform.displays();
    let Some(info) = displays.iter().find(|info| info.primary) else {
        // Without a window-server session (e.g. over SSH) there are no displays.
        assert!(!require_sck(), "a primary display is required");
        return;
    };
    assert!(info.width_px >= 2 && info.height_px >= 2);
    let unknown = (0..=u32::MAX)
        .find(|id| displays.iter().all(|info| info.id != *id))
        .expect("an unused display ID exists");
    let rect = PixelRect {
        x: 0,
        y: 0,
        w: 2,
        h: 2,
    };
    let cfg = CaptureConfig {
        fps: 30,
        max_width: 2,
        max_height: 2,
    };
    let rejected = |display, rect, cfg| {
        let (tx, _rx) = mpsc::channel(1);
        match platform.capture_region(display, rect, cfg, tx) {
            Err(PlatformError::Platform(message)) => message,
            Err(error) => panic!("expected a platform validation error, got {error}"),
            Ok(_) => panic!("invalid crop input must be rejected before starting SCK"),
        }
    };

    assert_eq!(rejected(unknown, rect, cfg), "unknown display");
    for invalid in [
        PixelRect {
            x: info.width_px - 1,
            ..rect
        },
        PixelRect {
            y: info.height_px - 1,
            ..rect
        },
        PixelRect {
            x: u32::MAX,
            ..rect
        },
        PixelRect {
            y: u32::MAX,
            ..rect
        },
        PixelRect { w: 1, ..rect },
        PixelRect { h: 1, ..rect },
        PixelRect { w: 0, ..rect },
        PixelRect { h: 0, ..rect },
    ] {
        assert!(rejected(info.id, invalid, cfg).contains("outside"));
    }
    for size in [0, 1] {
        for invalid in [
            CaptureConfig {
                max_width: size,
                ..cfg
            },
            CaptureConfig {
                max_height: size,
                ..cfg
            },
        ] {
            assert_eq!(rejected(info.id, rect, invalid), "invalid crop output size");
        }
    }
    for fps in [0, 121] {
        assert_eq!(
            rejected(info.id, rect, CaptureConfig { fps, ..cfg }),
            "capture fps must be 1..120"
        );
    }
}

#[tokio::test]
async fn region_frames_have_output_size() {
    if !require_sck() {
        return;
    }
    let platform = MacPlatform::new();
    let info = primary_display(&platform);
    let rect = PixelRect {
        x: (info.width_px / 4) & !15,
        y: (info.height_px / 4) & !15,
        w: (info.width_px / 2) & !15,
        h: (info.height_px / 2) & !15,
    };
    let cfg = CaptureConfig {
        fps: 30,
        max_width: (rect.w / 2) & !1,
        max_height: (rect.h / 2) & !1,
    };
    let _activity = ScreenActivity::start();
    let (tx, mut rx) = mpsc::channel(16);
    let handle = platform
        .capture_region(info.id, rect, cfg, tx)
        .unwrap_or_else(|error| panic!("RS_REQUIRE_SCK=1: region capture failed: {error}"));
    let size = (cfg.max_width, cfg.max_height);
    receive_frames(&mut rx, info.id, size, 3).await;
    stop_capture(handle, &mut rx, info.id, size).await;
}

#[tokio::test]
async fn full_capture_unchanged() {
    if !require_sck() {
        return;
    }
    let platform = MacPlatform::new();
    let info = primary_display(&platform);
    let cfg = CaptureConfig {
        fps: 30,
        max_width: 1280,
        max_height: 0,
    };
    let size = dimensions(&info, cfg.max_width, cfg.max_height).expect("full capture dimensions");
    let _activity = ScreenActivity::start();
    let (tx, mut rx) = mpsc::channel(16);
    let handle = platform
        .capture(info.id, cfg, tx)
        .unwrap_or_else(|error| panic!("RS_REQUIRE_SCK=1: full capture failed: {error}"));
    receive_frames(&mut rx, info.id, size, 3).await;
    stop_capture(handle, &mut rx, info.id, size).await;
}
