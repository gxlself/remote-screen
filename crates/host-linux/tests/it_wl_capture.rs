#![cfg(feature = "desktop-it")]

use std::fs::{File, TryLockError};
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use host_core::platform::{CaptureConfig, CapturedFrame, DisplayInfo, PlatformError};
use host_linux::frame::LinuxFrame;
use host_linux::portal::{PortalOptions, PortalSession};
use host_linux::pw::PwLib;
use host_linux::wl_capture::{WaylandCapture, WlStreamSource};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};

const DEADLINE: Duration = Duration::from_secs(10);
const CONFIG: CaptureConfig = CaptureConfig {
    fps: 30,
    max_width: 0,
    max_height: 0,
};

struct StateDir(PathBuf);

impl StateDir {
    fn new() -> Self {
        let path = PathBuf::from(std::env::var_os("TMPDIR").expect("TMPDIR on the test volume"))
            .join(format!("rs-it-wl-capture-{}", std::process::id()));
        std::fs::create_dir_all(&path).expect("portal state directory");
        Self(path)
    }
}

impl Drop for StateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// nextest runs each test in its own process. A process-local mutex would allow
// the resize cases to change another case's desktop underneath it.
async fn desktop_lock() -> File {
    let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("private desktop runtime"))
        .join("it-wl-capture.lock");
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .expect("desktop test lock");
    timeout(DEADLINE, async {
        loop {
            match file.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) => sleep(Duration::from_millis(20)).await,
                Err(error) => panic!("desktop lock: {error}"),
            }
        }
    })
    .await
    .expect("another Wayland capture test did not release the desktop");
    file
}

async fn resolution(width: u32, height: u32) {
    let output = timeout(
        DEADLINE,
        Command::new("swaymsg")
            .args([
                "-t",
                "command",
                &format!("output HEADLESS-1 resolution {width}x{height}"),
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("swaymsg timeout")
    .expect("swaymsg");
    assert!(output.status.success(), "swaymsg: {output:?}");
    let replies: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("sway IPC reply");
    assert!(
        replies
            .as_array()
            .expect("sway reply array")
            .iter()
            .all(|reply| reply["success"] == true),
        "sway command failed: {replies}"
    );
}

async fn with_capture<F, Fut>(test: F)
where
    F: FnOnce(WaylandCapture) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    assert_eq!(std::env::var("RS_IT_LAYER").as_deref(), Ok("sway"));
    let _lock = desktop_lock().await;
    resolution(1280, 720).await;
    let dir = StateDir::new();
    let conn = timeout(DEADLINE, zbus::Connection::session())
        .await
        .expect("session bus timeout")
        .expect("session bus");
    let session = timeout(
        DEADLINE,
        PortalSession::open(&conn, &PortalOptions::new(dir.0.clone(), true)),
    )
    .await
    .expect("portal timeout")
    .expect("ScreenCast portal grant");
    let mut sources = Vec::new();
    for display in &session.grant().displays {
        let fd = timeout(DEADLINE, session.open_pipewire_remote())
            .await
            .expect("PipeWire remote timeout")
            .expect("fresh portal PipeWire fd");
        sources.push(WlStreamSource {
            display_id: display.id,
            node_id: display.node_id,
            fd,
        });
    }
    let capture = WaylandCapture::new(PwLib::load().expect("PipeWire"), session.grant(), sources);
    // Keep cleanup outside the test task so a failed assertion still restores
    // HEADLESS-1 and closes the portal session before releasing the desktop lock.
    let mut task = tokio::spawn(test(capture));
    let result = timeout(DEADLINE, &mut task).await;
    if result.is_err() {
        task.abort();
        let _ = timeout(DEADLINE, task).await;
    }
    resolution(1280, 720).await;
    timeout(DEADLINE, session.close())
        .await
        .expect("portal close timeout")
        .expect("portal close");
    if let Err(error) = result.expect("capture test timed out") {
        std::panic::resume_unwind(error.into_panic());
    }
}

async fn display_size(
    rx: &mut mpsc::Receiver<Vec<DisplayInfo>>,
    width: u32,
    height: u32,
) -> DisplayInfo {
    timeout(DEADLINE, async {
        loop {
            let displays = rx.recv().await.expect("display watch closed");
            if displays.len() == 1
                && displays[0].width_px == width
                && displays[0].height_px == height
            {
                return displays[0].clone();
            }
        }
    })
    .await
    .expect("display dimensions did not update")
}

async fn frame_size(
    rx: &mut mpsc::Receiver<CapturedFrame>,
    width: u32,
    height: u32,
) -> CapturedFrame {
    timeout(DEADLINE, async {
        loop {
            let frame = rx.recv().await.expect("frame sink closed");
            if frame.width == width && frame.height == height {
                return frame;
            }
        }
    })
    .await
    .expect("frame dimensions did not update")
}

#[tokio::test(flavor = "multi_thread")]
async fn display_list() {
    with_capture(|capture| async move {
        let (tx, mut rx) = mpsc::channel(8);
        capture.watch_displays(tx);
        let display = display_size(&mut rx, 1280, 720).await;
        assert_eq!(capture.displays(), vec![display]);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn first_frame() {
    with_capture(|capture| async move {
        let id = capture.displays()[0].id;
        let (tx, mut rx) = mpsc::channel(4);
        let _handle = capture.capture(id, CONFIG, tx).expect("capture");
        let frame = frame_size(&mut rx, 1280, 720).await;
        assert_eq!(frame.display_id, id);
        assert!(frame.capture_ts_us > 0);
        let native = LinuxFrame::of(&frame).expect("Linux frame");
        assert_eq!((native.width, native.height), (1280, 720));
        assert_eq!(native.stride, 5120);
        assert!(native.generation >= 1);
        assert_eq!(native.row(719).expect("last row").len(), 5120);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn resize() {
    with_capture(|capture| async move {
        let id = capture.displays()[0].id;
        let (tx, mut frames) = mpsc::channel(4);
        let _handle = capture.capture(id, CONFIG, tx).expect("capture");
        let first = frame_size(&mut frames, 1280, 720).await;
        let generation = LinuxFrame::of(&first).expect("Linux frame").generation;
        let (tx, mut displays) = mpsc::channel(8);
        capture.watch_displays(tx);
        resolution(1024, 768).await;
        assert_eq!(display_size(&mut displays, 1024, 768).await.id, id);
        let resized = frame_size(&mut frames, 1024, 768).await;
        let native = LinuxFrame::of(&resized).expect("Linux frame");
        assert!(native.generation > generation);
        assert_eq!(native.stride, 4096);
        assert_eq!(native.row(767).expect("last row").len(), 4096);
        // The frame held across renegotiation must still own its old storage.
        assert_eq!(
            LinuxFrame::of(&first).unwrap().row(719).unwrap().len(),
            5120
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn fanout_and_inactive() {
    with_capture(|capture| async move {
        let id = capture.displays()[0].id;
        assert!(!capture.stats(id).expect("stats").active);
        let (tx1, mut rx1) = mpsc::channel(4);
        let (tx2, mut rx2) = mpsc::channel(4);
        let handle1 = capture.capture(id, CONFIG, tx1).expect("first sink");
        let handle2 = capture.capture(id, CONFIG, tx2).expect("second sink");
        // Ensure damage after both sinks are attached, even on a static desktop.
        resolution(1024, 768).await;
        frame_size(&mut rx1, 1024, 768).await;
        frame_size(&mut rx2, 1024, 768).await;
        drop(handle1);
        assert!(capture.stats(id).expect("stats").active);
        resolution(1280, 720).await;
        frame_size(&mut rx2, 1280, 720).await;
        drop(handle2);
        assert!(!capture.stats(id).expect("stats").active);
        // Reuse the retained stream/fd and re-arm its first-frame gate.
        let (tx, mut rx) = mpsc::channel(4);
        let handle = capture.capture(id, CONFIG, tx).expect("reactivate");
        frame_size(&mut rx, 1280, 720).await;
        drop(handle);
        assert!(!capture.stats(id).expect("stats").active);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_display() {
    with_capture(|capture| async move {
        let (tx, _rx) = mpsc::channel(1);
        assert!(matches!(
            capture.capture(99, CONFIG, tx),
            Err(PlatformError::Platform(message)) if message == "display not available"
        ));
        assert!(capture.stats(99).is_none());
    })
    .await;
}
