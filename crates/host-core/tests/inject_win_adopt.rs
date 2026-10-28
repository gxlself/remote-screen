use host_core::inject::keymap_win::WinKey;
use host_core::inject::win_plan::{ScreenRect, WinButton, WinInputOp, WinPlanner};
use rsp_core::hid::{Button, InputMsg, Mods};

const SCREEN: ScreenRect = ScreenRect {
    left: 0,
    top: 0,
    width: 1920,
    height: 1080,
};

fn key(scan: u16, up: bool) -> WinInputOp {
    WinInputOp::Key {
        key: WinKey {
            scan,
            extended: false,
            vk: 0,
        },
        up,
    }
}

fn key_msg(usage: u16, down: bool, mods: Mods) -> InputMsg {
    InputMsg::Key { usage, down, mods }
}

fn plan(planner: &mut WinPlanner, msg: &InputMsg) -> Vec<WinInputOp> {
    planner.plan(msg, SCREEN, SCREEN)
}

#[test]
fn adoption_releases_an_unmatched_enter() {
    let mut planner = WinPlanner::for_new_process();

    assert_eq!(
        plan(&mut planner, &key_msg(0x28, false, Mods::NONE)),
        vec![key(0x1C, true)]
    );
    assert_eq!(planner.adopted_ups(), 1);
    assert_eq!(planner.stats().stray_up, 0);
}

#[test]
fn adoption_releases_an_unmatched_left_control() {
    let mut planner = WinPlanner::for_new_process();

    assert_eq!(
        plan(&mut planner, &key_msg(0xE0, false, Mods::NONE)),
        vec![key(0x1D, true)]
    );
    assert_eq!(planner.adopted_ups(), 1);
    assert_eq!(planner.stats().stray_up, 0);
}

#[test]
fn adoption_releases_an_unmatched_left_button() {
    let mut planner = WinPlanner::for_new_process();

    let ops = plan(
        &mut planner,
        &InputMsg::MouseButton {
            button: Button::Left,
            down: false,
            x: 0.5,
            y: 0.5,
            clicks: 1,
        },
    );
    assert_eq!(ops.len(), 2);
    assert!(matches!(ops[0], WinInputOp::MoveAbs { .. }));
    assert_eq!(
        ops[1],
        WinInputOp::Button {
            button: WinButton::Left,
            up: true,
        }
    );
    assert_eq!(planner.adopted_ups(), 1);
    assert_eq!(planner.stats().stray_up, 0);
}

#[test]
fn a_key_down_ends_adoption() {
    let mut planner = WinPlanner::for_new_process();

    assert_eq!(
        plan(&mut planner, &key_msg(0x04, true, Mods::NONE)),
        vec![key(0x1E, false)]
    );
    assert!(plan(&mut planner, &key_msg(0x28, false, Mods::NONE)).is_empty());
    assert_eq!(planner.stats().stray_up, 1);
    assert_eq!(planner.adopted_ups(), 0);
}

#[test]
fn non_press_events_do_not_end_adoption() {
    let mut planner = WinPlanner::for_new_process();

    assert!(plan(&mut planner, &InputMsg::MouseMove { x: 0.5, y: 0.5 }).len() == 1);
    assert!(plan(&mut planner, &InputMsg::ReleaseAll).is_empty());
    assert_eq!(
        plan(&mut planner, &key_msg(0x28, false, Mods::NONE)),
        vec![key(0x1C, true)]
    );
    assert_eq!(planner.adopted_ups(), 1);
}

#[test]
fn held_snapshot_tracks_keys_modifiers_and_buttons() {
    let mut planner = WinPlanner::new();

    plan(&mut planner, &key_msg(0x04, true, Mods::NONE));
    plan(&mut planner, &key_msg(0x05, true, Mods::NONE));
    plan(&mut planner, &key_msg(0x06, true, Mods::LCTRL));
    plan(
        &mut planner,
        &InputMsg::MouseButton {
            button: Button::Right,
            down: true,
            x: 0.25,
            y: 0.75,
            clicks: 1,
        },
    );

    let held = planner.held();
    assert_eq!(held.keys, vec![0x04, 0x05, 0x06]);
    assert_eq!(held.mods, Mods::LCTRL);
    assert_eq!(held.buttons, vec![Button::Right]);

    planner.release_all();
    assert!(planner.held().is_empty());
}

#[test]
fn new_planner_keeps_stray_up_behavior() {
    let mut planner = WinPlanner::new();

    assert!(plan(&mut planner, &key_msg(0x28, false, Mods::NONE)).is_empty());
    assert_eq!(planner.stats().stray_up, 1);
    assert_eq!(planner.adopted_ups(), 0);
}
