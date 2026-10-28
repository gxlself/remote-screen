//! M1 bidirectional text clipboard synchronization over a real session.

use std::time::Duration;

use host_core::platform::ClipboardItem as PlatformClipboardItem;
use host_fake::harness::{
    app_identity, direct_params, open, wait_state, TestHost, TestHostOptions,
};
use host_fake::FakePlatform;
use rsp_client::clipboard::ClipboardClientError;
use rsp_client::session::{SessionEvent, SessionStateKind};
use rsp_core::clipboard::{ClipboardKind, MAX_TEXT_BYTES};
use rsp_core::pb;
use rsp_core::trust::Permission;
use tokio::time::{sleep, timeout};

const CLIPBOARD_TIMEOUT: Duration = Duration::from_secs(3);

fn platform_text(text: &str) -> PlatformClipboardItem {
    let data = text.as_bytes().to_vec();
    PlatformClipboardItem {
        kind: pb::clipboard_header::Kind::Text,
        sha256: host_core::session::clipboard::sha256(&data),
        data,
    }
}

async fn streaming_session(
    platform: FakePlatform,
    permission: Permission,
) -> (
    TestHost,
    FakePlatform,
    rsp_client::SessionHandle,
    rsp_client::EventStream,
) {
    let host = TestHost::start_with_platform(platform.clone(), TestHostOptions::default()).await;
    let app = app_identity();
    host.pair(&app, permission);
    let (session, mut events) = open(direct_params(&host, app)).await.expect("open session");
    assert!(wait_state(&mut events, SessionStateKind::Streaming, CLIPBOARD_TIMEOUT).await);
    (host, platform, session, events)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_to_app_emits_text_clipboard_event() {
    let (host, platform, session, mut events) =
        streaming_session(FakePlatform::new(), Permission::Control).await;
    platform.set_clipboard(Some(platform_text("RS-CLIP-1")));
    let item = timeout(CLIPBOARD_TIMEOUT, async {
        loop {
            if let Some(SessionEvent::Clipboard(item)) = events.recv().await {
                break item;
            }
        }
    })
    .await
    .expect("clipboard event within 3 seconds");
    assert_eq!(item.kind, ClipboardKind::Text);
    assert_eq!(item.data.as_ref(), b"RS-CLIP-1");
    session.close();
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_to_host_writes_text_clipboard() {
    let (host, platform, session, _events) =
        streaming_session(FakePlatform::new(), Permission::Control).await;
    session
        .send_clipboard_text("RS-CLIP-2")
        .expect("controller can send clipboard text");
    timeout(CLIPBOARD_TIMEOUT, async {
        loop {
            if platform
                .clipboard_writes()
                .iter()
                .any(|item| item.data == b"RS-CLIP-2")
            {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Host clipboard write within 3 seconds");
    session.close();
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn viewer_cannot_send_clipboard_text() {
    let (host, platform, session, _events) =
        streaming_session(FakePlatform::new(), Permission::ViewOnly).await;
    let error = session
        .send_clipboard_text("RS-CLIP-viewer")
        .expect_err("viewer cannot send clipboard text");
    assert!(matches!(error, ClipboardClientError::NotController));
    sleep(Duration::from_millis(100)).await;
    assert!(platform.clipboard_writes().is_empty());
    session.close();
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_text_is_rejected_without_ending_session() {
    let (host, platform, session, _events) =
        streaming_session(FakePlatform::new(), Permission::Control).await;
    let text = "x".repeat(MAX_TEXT_BYTES + 1);
    let error = session
        .send_clipboard_text(&text)
        .expect_err("oversized text is rejected");
    assert!(matches!(error, ClipboardClientError::Codec(_)));
    assert!(platform.clipboard_writes().is_empty());
    assert_eq!(session.state(), SessionStateKind::Streaming);
    session.close();
    host.stop().await;
}
