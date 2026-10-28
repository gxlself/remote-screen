use host_core::{
    auth::Role,
    platform::{ClipboardItem as PlatformClipboardItem, Platform},
    session::clipboard::{self, sha256},
};
use rsp_core::{
    clipboard::{encode_stream, ClipboardItem, ClipboardKind},
    features, pb,
};
use std::time::Duration;
use tokio::time::timeout;

mod wire_support;
use wire_support::{client_hello, Dev, WireClient, WireHost};

const TEXT: &str = "hello";
const PHONE_TEXT: &str = "from phone";

fn platform_item(
    kind: pb::clipboard_header::Kind,
    data: impl Into<Vec<u8>>,
) -> PlatformClipboardItem {
    let data = data.into();
    PlatformClipboardItem {
        kind,
        sha256: sha256(&data),
        data,
    }
}

fn text_platform(text: &str) -> PlatformClipboardItem {
    platform_item(pb::clipboard_header::Kind::Text, text.as_bytes())
}

fn png_platform(data: impl Into<Vec<u8>>) -> PlatformClipboardItem {
    platform_item(pb::clipboard_header::Kind::Png, data)
}

fn text_item(text: &str) -> ClipboardItem {
    ClipboardItem::new(ClipboardKind::Text, text.as_bytes().to_vec()).expect("valid clipboard text")
}

fn png_item() -> ClipboardItem {
    ClipboardItem::new(ClipboardKind::Png, vec![0x89, b'P', b'N', b'G'])
        .expect("valid clipboard PNG")
}

async fn expect_clipboard(client: &WireClient) -> ClipboardItem {
    let (kind, mut recv) = client.accept_typed().await;
    assert_eq!(kind, clipboard::CLIPBOARD_STREAM_TYPE);
    timeout(Duration::from_secs(3), clipboard::read_stream(&mut recv))
        .await
        .expect("clipboard stream did not finish")
        .expect("read clipboard stream")
}

async fn wait_for_controller_clipboard(host: &WireHost, client: &WireClient) {
    for attempt in 0..50 {
        let text = format!("controller-ready-{attempt}");
        host.platform.set_clipboard(Some(text_platform(&text)));
        if let Ok(Ok(mut recv)) =
            timeout(Duration::from_millis(100), client.conn.accept_uni()).await
        {
            let mut kind = [0];
            if let Ok(Ok(())) = timeout(Duration::from_secs(3), recv.read_exact(&mut kind)).await {
                assert_eq!(
                    kind[0],
                    clipboard::CLIPBOARD_STREAM_TYPE,
                    "unexpected stream type"
                );
                let item = timeout(Duration::from_secs(3), clipboard::read_stream(&mut recv))
                    .await
                    .expect("clipboard stream did not finish")
                    .expect("read clipboard stream");
                assert_eq!(item.kind, ClipboardKind::Text);
                assert_eq!(item.data.as_ref(), text.as_bytes());
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("controller never received the clipboard change");
}

async fn send_clipboard(client: &WireClient, item: &ClipboardItem) {
    let mut send = client.open_typed(clipboard::CLIPBOARD_STREAM_TYPE).await;
    send.write_all(&encode_stream(item).expect("encode clipboard stream"))
        .await
        .expect("write clipboard stream");
    send.finish().expect("finish clipboard stream");
}

async fn assert_no_clipboard(client: &WireClient) {
    assert!(
        timeout(Duration::from_millis(500), client.conn.accept_uni())
            .await
            .is_err(),
        "unexpected clipboard stream"
    );
}

async fn connect_viewer(host: &WireHost, dev: Dev) -> WireClient {
    let mut client = host.connect_as(dev, 0).await;
    let (_, auth) = client
        .hello_with(pb::ClientHello {
            intent: pb::Intent::View as i32,
            ..client_hello()
        })
        .await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    client
}

#[tokio::test]
async fn local_change_is_pushed_to_the_controller_with_fin() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;

    wait_for_controller_clipboard(&host, &a).await;
    host.platform.set_clipboard(Some(text_platform(TEXT)));
    let item = expect_clipboard(&a).await;
    assert_eq!(item.kind, ClipboardKind::Text);
    assert_eq!(item.data.as_ref(), TEXT.as_bytes());
}

#[tokio::test]
async fn viewers_do_not_receive_local_changes() {
    let host = WireHost::start().await;
    let viewer = connect_viewer(&host, Dev::B).await;

    host.platform.set_clipboard(Some(text_platform(TEXT)));
    assert_no_clipboard(&viewer).await;
}

#[tokio::test]
async fn phone_write_reaches_platform_without_echoing_back() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    wait_for_controller_clipboard(&host, &a).await;

    send_clipboard(&a, &text_item(PHONE_TEXT)).await;
    host.wait_until(|| {
        host.platform
            .clipboard_writes()
            .iter()
            .any(|item| item.data == PHONE_TEXT.as_bytes())
    })
    .await;

    assert_no_clipboard(&a).await;
}

#[tokio::test]
async fn viewer_writes_are_ignored() {
    let host = WireHost::start().await;
    let viewer = connect_viewer(&host, Dev::B).await;

    send_clipboard(&viewer, &text_item(PHONE_TEXT)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(host.platform.clipboard_writes().is_empty());
    assert!(host.platform.clipboard_get().is_none());
}

#[tokio::test]
async fn directions_are_hot_updated() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    wait_for_controller_clipboard(&host, &a).await;

    host.config
        .update(&serde_json::json!({"clipboard": {"to_phone": false}}))
        .expect("disable host to phone");
    tokio::time::sleep(Duration::from_millis(50)).await;
    host.platform.set_clipboard(Some(text_platform("blocked")));
    assert_no_clipboard(&a).await;

    host.config
        .update(&serde_json::json!({"clipboard": {"from_phone": false}}))
        .expect("disable phone to host");
    tokio::time::sleep(Duration::from_millis(50)).await;
    send_clipboard(&a, &text_item("disabled")).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(host.platform.clipboard_writes().is_empty());
}

#[tokio::test]
async fn demoted_controller_stops_receiving() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    wait_for_controller_clipboard(&host, &a).await;
    assert_eq!(host.conns()[0].role, Role::Controller);
    let session_id = host.conns()[0].session_id.clone();

    assert!(host.mgr.session_control().set_view_only(&session_id));
    host.wait_until(|| host.conns()[0].role == Role::Viewer)
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    host.platform.set_clipboard(Some(text_platform(TEXT)));
    assert_no_clipboard(&a).await;
}

#[tokio::test]
async fn images_require_the_image_feature_but_text_still_works() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    let (_, auth) = a
        .hello_with(pb::ClientHello {
            features: features::CLIPBOARD_TEXT,
            ..client_hello()
        })
        .await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    wait_for_controller_clipboard(&host, &a).await;

    host.platform
        .set_clipboard(Some(png_platform(vec![0x89, b'P', b'N', b'G'])));
    assert_no_clipboard(&a).await;

    host.platform.set_clipboard(Some(text_platform(TEXT)));
    let item = expect_clipboard(&a).await;
    assert_eq!(item.kind, ClipboardKind::Text);
    assert_eq!(item.data.as_ref(), TEXT.as_bytes());
}

#[tokio::test]
async fn server_hello_advertises_both_clipboard_features() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    let (server, _) = a.hello().await;
    let required = features::CLIPBOARD_TEXT | features::CLIPBOARD_IMAGE;
    assert_eq!(server.features & required, required);
}

#[tokio::test]
async fn image_feature_is_pushed_when_negotiated() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    wait_for_controller_clipboard(&host, &a).await;
    let writes = host.platform.clipboard_writes().len();

    let item = png_item();
    host.platform
        .set_clipboard(Some(png_platform(item.data.to_vec())));
    let received = expect_clipboard(&a).await;
    assert_eq!(received, item);

    send_clipboard(&a, &item).await;
    host.wait_until(|| host.platform.clipboard_writes().len() > writes)
        .await;
}
