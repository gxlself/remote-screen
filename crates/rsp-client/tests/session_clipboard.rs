use rsp_client::clipboard::text_item;
use rsp_client::session::CLIENT_FEATURES;
use rsp_core::clipboard::ClipboardKind;
use rsp_core::features::CLIPBOARD_TEXT;

#[test]
fn session_advertises_text_clipboard_and_builds_utf8_items() {
    assert_ne!(CLIENT_FEATURES & CLIPBOARD_TEXT, 0);
    let item = text_item("session clipboard").expect("valid UTF-8 text");
    assert_eq!(item.kind, ClipboardKind::Text);
    assert_eq!(item.data.as_ref(), b"session clipboard");
}
