#![cfg(target_os = "linux")]

#[path = "support/mock_portal.rs"]
mod mock_portal;

use std::{collections::HashMap, time::Duration};

use host_linux::{
    input_plan::DisplayGeom,
    portal_input::{PortalInjector, PortalTarget},
};
use mock_portal::{MockConfig, MockPortal, MockVal};
use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit};
use tokio::time::timeout;
use zbus::{OwnedObjectPath, OwnedVariant, Proxy, Str};

const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const RD: &str = "org.freedesktop.portal.RemoteDesktop";

type Options = HashMap<String, OwnedVariant>;

fn text(value: &str) -> OwnedVariant {
    Str::from(value).into()
}

async fn setup(devices: u32) -> (MockPortal, PortalInjector, PortalTarget) {
    let mock = MockPortal::start(MockConfig::gnome()).await;
    let client = mock.client().await;
    let sender = client
        .unique_name()
        .expect("mock client unique name")
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let rd = Proxy::new(&client, PORTAL, DESKTOP, RD)
        .await
        .expect("create remote desktop proxy");
    let options: Options = HashMap::from([
        ("handle_token".into(), text("create")),
        ("session_handle_token".into(), text("session")),
    ]);
    let _: OwnedObjectPath = rd
        .call("CreateSession", &(options,))
        .await
        .expect("create portal session request");
    let session = format!("{DESKTOP}/session/{sender}/session");
    let target = PortalTarget {
        display_id: 1,
        node_id: 42,
        geom: DisplayGeom {
            width: 1921.0,
            height: 1081.0,
        },
    };
    let injector =
        PortalInjector::new(client, session, devices, &tokio::runtime::Handle::current());
    (mock, injector, target)
}

async fn wait_for(mock: &MockPortal, member: &str, count: usize) {
    timeout(Duration::from_secs(5), async {
        loop {
            if mock.calls_to(member).len() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("mock portal call timeout");
}

#[tokio::test(flavor = "multi_thread")]
async fn move_and_click() {
    let (mock, injector, target) = setup(3).await;
    injector.inject(
        &InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.5,
            y: 0.25,
            clicks: 1,
        },
        target,
    );
    injector.flush().await;
    wait_for(&mock, "NotifyPointerButton", 1).await;

    let motion = mock.calls_to("NotifyPointerMotionAbsolute");
    assert_eq!(
        motion[0].args,
        [MockVal::U32(42), MockVal::F64(960.0), MockVal::F64(270.0)]
    );
    let button = mock.calls_to("NotifyPointerButton");
    assert_eq!(button[0].args, [MockVal::I32(0x110), MockVal::U32(1)]);
    let last = injector.last_pointer().expect("last pointer");
    assert_eq!((last.display_id, last.x, last.y), (1, 960.0, 270.0));
    assert_eq!(injector.stats().sent, 2);
    assert_eq!(injector.stats().queue_full, 0);
    assert_eq!(injector.stats().send_errors, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn keys_and_text() {
    let (mock, injector, target) = setup(3).await;
    injector.inject(
        &InputMsg::Key {
            usage: 0x04,
            down: true,
            mods: Mods::LCTRL,
        },
        target,
    );
    injector.inject(&InputMsg::Text("a".into()), target);
    injector.flush().await;

    wait_for(&mock, "NotifyKeyboardKeysym", 2).await;
    let keycodes = mock.calls_to("NotifyKeyboardKeycode");
    assert_eq!(keycodes[0].args, [MockVal::I32(29), MockVal::U32(1)]);
    assert_eq!(keycodes[1].args, [MockVal::I32(30), MockVal::U32(1)]);
    assert!(keycodes
        .iter()
        .any(|call| call.args == [MockVal::I32(29), MockVal::U32(0)]));
    assert!(keycodes
        .iter()
        .any(|call| call.args == [MockVal::I32(29), MockVal::U32(1)]));
    let keysyms = mock.calls_to("NotifyKeyboardKeysym");
    assert_eq!(keysyms[0].args, [MockVal::I32(0x61), MockVal::U32(1)]);
    assert_eq!(keysyms[1].args, [MockVal::I32(0x61), MockVal::U32(0)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn smooth_scroll_and_stop() {
    let (mock, injector, target) = setup(3).await;
    injector.inject(
        &InputMsg::Scroll {
            dx: 0.0,
            dy: 10.0,
            phase: ScrollPhase::Began,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Pixel,
        },
        target,
    );
    injector.flush().await;
    wait_for(&mock, "NotifyPointerAxis", 1).await;
    wait_for(&mock, "NotifyPointerAxis", 2).await;
    let calls = mock.calls_to("NotifyPointerAxis");
    assert_eq!(calls[0].args, [MockVal::F64(0.0), MockVal::F64(-10.0)]);
    assert_eq!(calls[0].options, []);
    assert_eq!(calls[1].args, [MockVal::F64(0.0), MockVal::F64(0.0)]);
    assert_eq!(calls[1].options, [("finish".into(), MockVal::Bool(true))]);
}

#[tokio::test(flavor = "multi_thread")]
async fn line_scroll_discrete() {
    let (mock, injector, target) = setup(3).await;
    injector.inject(
        &InputMsg::Scroll {
            dx: 0.0,
            dy: 3.0,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Line,
        },
        target,
    );
    injector.flush().await;
    wait_for(&mock, "NotifyPointerAxisDiscrete", 1).await;
    assert_eq!(
        mock.calls_to("NotifyPointerAxisDiscrete")[0].args,
        [MockVal::U32(0), MockVal::I32(-1)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_keyboard_device() {
    let (mock, injector, target) = setup(2).await;
    injector.inject(
        &InputMsg::Key {
            usage: 0x04,
            down: true,
            mods: Mods::NONE,
        },
        target,
    );
    injector.inject(&InputMsg::MouseMove { x: 0.5, y: 0.5 }, target);
    injector.flush().await;
    wait_for(&mock, "NotifyPointerMotionAbsolute", 1).await;
    assert!(mock.calls_to("NotifyKeyboardKeycode").is_empty());
    assert!(injector.stats().no_device >= 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn release_all_sends_ups() {
    let (mock, injector, target) = setup(3).await;
    injector.inject(
        &InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.1,
            y: 0.1,
            clicks: 1,
        },
        target,
    );
    injector.inject(
        &InputMsg::Key {
            usage: 0x04,
            down: true,
            mods: Mods::NONE,
        },
        target,
    );
    injector.release_all();
    injector.flush().await;
    wait_for(&mock, "NotifyPointerButton", 2).await;
    assert!(mock
        .calls_to("NotifyKeyboardKeycode")
        .iter()
        .any(|call| call.args == [MockVal::I32(30), MockVal::U32(0)]));
    assert!(mock
        .calls_to("NotifyPointerButton")
        .iter()
        .any(|call| call.args == [MockVal::I32(0x110), MockVal::U32(0)]));
}

#[tokio::test(flavor = "multi_thread")]
async fn wake_sends_zero_motion() {
    let (mock, injector, _target) = setup(3).await;
    injector.wake();
    injector.flush().await;
    wait_for(&mock, "NotifyPointerMotion", 1).await;
    assert_eq!(
        mock.calls_to("NotifyPointerMotion")[0].args,
        [MockVal::F64(0.0), MockVal::F64(0.0)]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn order_preserved() {
    let (mock, injector, target) = setup(3).await;
    for i in 0..200 {
        injector.inject(
            &InputMsg::MouseMove {
                x: i as f32 / 200.0,
                y: 0.5,
            },
            target,
        );
    }
    injector.flush().await;
    wait_for(&mock, "NotifyPointerMotionAbsolute", 200).await;
    let calls = mock.calls_to("NotifyPointerMotionAbsolute");
    assert_eq!(calls.len(), 200);
    let mut previous = f64::NEG_INFINITY;
    for call in calls {
        let MockVal::F64(x) = call.args[1] else {
            panic!("motion x is not f64");
        };
        assert!(x > previous);
        previous = x;
    }
}
