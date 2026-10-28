use host_core::inject::keymap_win::WinKey;
use host_core::inject::win_plan::{
    input_from_m0, to_virtual_desk, ScreenRect, WinButton, WinInputOp, WinInputStats, WinPlanner,
    DEFAULT_WHEEL_UNITS_PER_PIXEL, HWHEEL_SIGN, RS_EXTRA_INFO, WHEEL_DELTA, WHEEL_UNITS_PER_LINE,
};
use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit, TouchPhase};
use rsp_core::input::{InputEvent, MouseButton};

const NONE: Mods = Mods::NONE;
const LCTRL: Mods = Mods::LCTRL;
const LSHIFT: Mods = Mods::LSHIFT;

fn scan(value: u16) -> WinKey {
    WinKey {
        scan: value,
        extended: false,
        vk: 0,
    }
}

fn extended(value: u16) -> WinKey {
    WinKey {
        scan: value,
        extended: true,
        vk: 0,
    }
}

fn key(value: u16, up: bool) -> WinInputOp {
    WinInputOp::Key {
        key: scan(value),
        up,
    }
}

fn key_msg(usage: u16, down: bool, mods: Mods) -> InputMsg {
    InputMsg::Key { usage, down, mods }
}

fn unicode(unit: u16, up: bool) -> WinInputOp {
    WinInputOp::Unicode { unit, up }
}

fn scroll(dx: f32, dy: f32, phase: ScrollPhase, unit: ScrollUnit) -> InputMsg {
    InputMsg::Scroll {
        dx,
        dy,
        phase,
        momentum: ScrollPhase::None,
        unit,
    }
}

fn line(dx: f32, dy: f32) -> InputMsg {
    scroll(dx, dy, ScrollPhase::None, ScrollUnit::Line)
}

fn pixel(dx: f32, dy: f32) -> InputMsg {
    scroll(dx, dy, ScrollPhase::None, ScrollUnit::Pixel)
}

fn primary() -> ScreenRect {
    ScreenRect {
        left: 0,
        top: 0,
        width: 1920,
        height: 1080,
    }
}

#[test]
fn constants_match_the_windows_contract() {
    assert_eq!(RS_EXTRA_INFO, 0x5253);
    assert_eq!(WHEEL_DELTA, 120);
    assert_eq!(WHEEL_UNITS_PER_LINE, 40.0);
    assert_eq!(DEFAULT_WHEEL_UNITS_PER_PIXEL, 1.2);
    assert_eq!(HWHEEL_SIGN, -1);
    assert_eq!(
        WinInputStats::default(),
        WinInputStats {
            unknown_usage: 0,
            stray_up: 0,
            overflow: 0,
            dropped: 0,
        }
    );
}

#[test]
fn single_display_coordinates_cover_the_whole_axis() {
    let screen = primary();
    assert_eq!(to_virtual_desk(0.0, 0.0, screen, screen), (0, 0));
    assert_eq!(to_virtual_desk(1.0, 1.0, screen, screen), (65535, 65535));
    assert_eq!(to_virtual_desk(0.5, 0.5, screen, screen), (32785, 32798));
}

#[test]
fn multi_display_coordinates_are_relative_to_the_virtual_desktop() {
    let desk = ScreenRect {
        left: -1280,
        top: 0,
        width: 3200,
        height: 1080,
    };
    let main = primary();
    let secondary = ScreenRect {
        left: -1280,
        top: 0,
        width: 1280,
        height: 1024,
    };

    assert_eq!(to_virtual_desk(0.0, 0.0, main, desk), (26222, 0));
    assert_eq!(to_virtual_desk(1.0, 1.0, main, desk), (65535, 65535));
    assert_eq!(to_virtual_desk(0.0, 0.0, secondary, desk), (0, 0));
    assert_eq!(to_virtual_desk(1.0, 1.0, secondary, desk), (26202, 62134));
    assert_eq!(to_virtual_desk(1.5, f32::NAN, main, desk), (65535, 0));
    assert_eq!(to_virtual_desk(f32::NAN, 1.5, secondary, desk), (0, 62134));
}

#[test]
fn ordinary_key_down_repeats_and_up_is_balanced() {
    let mut planner = WinPlanner::new();
    let msg = key_msg(0x04, true, NONE);
    assert_eq!(
        planner.plan(&msg, primary(), primary()),
        vec![key(0x1E, false)]
    );
    assert_eq!(
        planner.plan(&msg, primary(), primary()),
        vec![key(0x1E, false)]
    );
    assert_eq!(
        planner.plan(&key_msg(0x04, false, NONE), primary(), primary()),
        vec![key(0x1E, true)]
    );
    assert!(planner
        .plan(&key_msg(0x04, false, NONE), primary(), primary())
        .is_empty());
    assert_eq!(planner.stats().stray_up, 1);
}

#[test]
fn ordinary_keys_synchronize_modifiers_before_the_key() {
    let mut planner = WinPlanner::new();
    let both = LCTRL | LSHIFT;
    assert_eq!(
        planner.plan(&key_msg(0x04, true, both), primary(), primary()),
        vec![key(0x1D, false), key(0x2A, false), key(0x1E, false)]
    );
    assert_eq!(
        planner.plan(&key_msg(0x04, false, NONE), primary(), primary()),
        vec![key(0x1D, true), key(0x2A, true), key(0x1E, true)]
    );
}

#[test]
fn a_modifiers_own_bit_wins_over_contradictory_mods() {
    let mut planner = WinPlanner::new();
    assert_eq!(
        planner.plan(&key_msg(0xE0, true, NONE), primary(), primary()),
        vec![key(0x1D, false)]
    );
    assert_eq!(
        planner.plan(&key_msg(0xE0, false, LCTRL), primary(), primary()),
        vec![key(0x1D, true)]
    );
}

#[test]
fn extended_keys_keep_their_extended_flag() {
    let mut planner = WinPlanner::new();
    assert_eq!(
        planner.plan(&key_msg(0x50, true, NONE), primary(), primary()),
        vec![WinInputOp::Key {
            key: extended(0x4B),
            up: false,
        }]
    );
}

#[test]
fn unknown_usages_are_dropped_without_synchronizing_modifiers() {
    let mut planner = WinPlanner::new();
    assert!(planner
        .plan(&key_msg(0x66, true, LCTRL), primary(), primary())
        .is_empty());
    assert_eq!(planner.stats().unknown_usage, 1);
}

#[test]
fn the_thirty_third_held_key_overflows() {
    let mut planner = WinPlanner::new();
    for usage in 0x04..=0x24 {
        let ops = planner.plan(&key_msg(usage, true, NONE), primary(), primary());
        if usage == 0x24 {
            assert!(ops.is_empty(), "usage {usage:#06x}");
        } else {
            assert_eq!(ops.len(), 1, "usage {usage:#06x}");
        }
    }
    assert_eq!(planner.stats().overflow, 1);
}

#[test]
fn text_releases_modifiers_emits_utf16_units_and_restores_modifiers() {
    let mut planner = WinPlanner::new();
    assert_eq!(
        planner.plan(&key_msg(0xE1, true, NONE), primary(), primary()),
        vec![key(0x2A, false)]
    );

    let ops = planner.plan(&InputMsg::Text("a😀\r\nb\t".into()), primary(), primary());
    assert_eq!(
        ops,
        vec![
            key(0x2A, true),
            unicode(0x0061, false),
            unicode(0x0061, true),
            unicode(0xD83D, false),
            unicode(0xD83D, true),
            unicode(0xDE00, false),
            unicode(0xDE00, true),
            key(0x1C, false),
            key(0x1C, true),
            unicode(0x0062, false),
            unicode(0x0062, true),
            key(0x0F, false),
            key(0x0F, true),
            key(0x2A, false),
        ]
    );
    assert_eq!(planner.stats().dropped, 0);
}

#[test]
fn text_control_characters_are_counted_as_dropped() {
    let mut planner = WinPlanner::new();
    let ops = planner.plan(&InputMsg::Text("a\u{1b}b".into()), primary(), primary());
    assert_eq!(
        ops,
        vec![
            unicode(0x0061, false),
            unicode(0x0061, true),
            unicode(0x0062, false),
            unicode(0x0062, true),
        ]
    );
    assert_eq!(planner.stats().dropped, 1);
}

#[test]
fn mouse_buttons_move_first_and_track_button_state() {
    let mut planner = WinPlanner::new();
    assert_eq!(
        planner.plan(
            &InputMsg::MouseButton {
                button: Button::Back,
                down: true,
                x: 0.5,
                y: 0.5,
                clicks: 1,
            },
            primary(),
            primary(),
        ),
        vec![
            WinInputOp::MoveAbs { x: 32785, y: 32798 },
            WinInputOp::Button {
                button: WinButton::X1,
                up: false,
            },
        ]
    );

    assert_eq!(
        planner.plan(
            &InputMsg::MouseButton {
                button: Button::Right,
                down: false,
                x: 0.0,
                y: 0.0,
                clicks: 1,
            },
            primary(),
            primary(),
        ),
        vec![WinInputOp::MoveAbs { x: 0, y: 0 }]
    );
    assert_eq!(planner.stats().stray_up, 1);
}

#[test]
fn scroll_converts_lines_pixels_and_resets_on_began() {
    let mut planner = WinPlanner::new();
    assert_eq!(
        planner.plan(&line(0.0, 1.0), primary(), primary()),
        vec![WinInputOp::Wheel { delta: 40 }]
    );
    assert_eq!(
        planner.plan(&line(1.0, 0.0), primary(), primary()),
        vec![WinInputOp::HWheel { delta: -40 }]
    );
    assert_eq!(
        planner.plan(&line(0.0, -3.0), primary(), primary()),
        vec![WinInputOp::Wheel { delta: -120 }]
    );
    assert_eq!(
        planner.plan(&line(0.0, 1e12), primary(), primary()),
        vec![WinInputOp::Wheel { delta: i32::MAX }]
    );

    let mut planner = WinPlanner::with_wheel_units_per_pixel(1.0);
    assert!(planner
        .plan(&pixel(0.0, 0.5), primary(), primary())
        .is_empty());
    assert_eq!(
        planner.plan(&pixel(0.0, 0.5), primary(), primary()),
        vec![WinInputOp::Wheel { delta: 1 }]
    );
    assert!(planner
        .plan(&pixel(0.0, 0.5), primary(), primary())
        .is_empty());
    assert!(planner
        .plan(
            &scroll(0.0, 0.5, ScrollPhase::Began, ScrollUnit::Pixel),
            primary(),
            primary(),
        )
        .is_empty());
}

#[test]
fn non_finite_scroll_is_dropped_without_touching_the_remainder() {
    let mut planner = WinPlanner::with_wheel_units_per_pixel(1.0);
    assert!(planner
        .plan(&pixel(0.0, 0.5), primary(), primary())
        .is_empty());
    assert!(planner
        .plan(&pixel(0.0, f32::NAN), primary(), primary())
        .is_empty());
    assert_eq!(planner.stats().dropped, 1);
    assert_eq!(
        planner.plan(&pixel(0.0, 0.5), primary(), primary()),
        vec![WinInputOp::Wheel { delta: 1 }]
    );
}

#[test]
fn touch_is_dropped() {
    let mut planner = WinPlanner::new();
    let ops = planner.plan(
        &InputMsg::Touch {
            id: 1,
            phase: TouchPhase::Began,
            x: 0.5,
            y: 0.5,
            pressure: 0.5,
        },
        primary(),
        primary(),
    );
    assert!(ops.is_empty());
    assert_eq!(planner.stats().dropped, 1);
}

#[test]
fn release_all_uses_reverse_key_order_then_modifiers_then_buttons() {
    let mut planner = WinPlanner::new();
    for msg in [
        key_msg(0x04, true, NONE),
        key_msg(0x05, true, NONE),
        key_msg(0xE0, true, NONE),
        InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.0,
            y: 0.0,
            clicks: 1,
        },
    ] {
        let _ = planner.plan(&msg, primary(), primary());
    }
    assert_eq!(
        planner.release_all(),
        vec![
            key(0x30, true),
            key(0x1E, true),
            key(0x1D, true),
            WinInputOp::Button {
                button: WinButton::Left,
                up: true,
            },
        ]
    );
    assert!(planner.release_all().is_empty());

    let mut planner = WinPlanner::new();
    for msg in [
        key_msg(0x04, true, NONE),
        key_msg(0x05, true, NONE),
        key_msg(0xE0, true, NONE),
        InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.0,
            y: 0.0,
            clicks: 1,
        },
    ] {
        let _ = planner.plan(&msg, primary(), primary());
    }
    assert_eq!(
        planner.plan(&InputMsg::ReleaseAll, primary(), primary()),
        vec![
            key(0x30, true),
            key(0x1E, true),
            key(0x1D, true),
            WinInputOp::Button {
                button: WinButton::Left,
                up: true,
            },
        ]
    );
}

#[test]
fn m0_scroll_conversion_uses_pixels_and_no_phase() {
    assert_eq!(
        input_from_m0(&InputEvent::Scroll { dx: 0.0, dy: 3.0 }),
        InputMsg::Scroll {
            dx: 0.0,
            dy: 3.0,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Pixel,
        }
    );
    assert_eq!(
        input_from_m0(&InputEvent::MouseButton {
            button: MouseButton::Left,
            down: true,
            x: 0.25,
            y: 0.75,
            clicks: 2,
        }),
        InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.25,
            y: 0.75,
            clicks: 2,
        }
    );
}
