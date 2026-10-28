use host_linux::input_plan::{
    DisplayGeom, LinuxInputOp as Op, LinuxInputStats, LinuxPlanner, ScrollMode, WheelAxis,
    MAX_PRESSED, SCROLL_STOP_US,
};
use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit, TouchPhase};

const G: DisplayGeom = DisplayGeom {
    width: 1921.0,
    height: 1081.0,
};

fn key(usage: u16, down: bool, mods: Mods) -> InputMsg {
    InputMsg::Key { usage, down, mods }
}

fn button(button: Button, down: bool) -> InputMsg {
    InputMsg::MouseButton {
        button,
        down,
        x: 0.0,
        y: 0.0,
        clicks: 1,
    }
}

fn scroll(
    dx: f32,
    dy: f32,
    phase: ScrollPhase,
    momentum: ScrollPhase,
    unit: ScrollUnit,
) -> InputMsg {
    InputMsg::Scroll {
        dx,
        dy,
        phase,
        momentum,
        unit,
    }
}

fn pixel(dx: f32, dy: f32) -> InputMsg {
    scroll(
        dx,
        dy,
        ScrollPhase::None,
        ScrollPhase::None,
        ScrollUnit::Pixel,
    )
}

fn line(dx: f32, dy: f32) -> InputMsg {
    scroll(
        dx,
        dy,
        ScrollPhase::None,
        ScrollPhase::None,
        ScrollUnit::Line,
    )
}

#[test]
fn move_scales_and_clamps() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    assert_eq!(
        planner.plan(&InputMsg::MouseMove { x: 0.5, y: 0.25 }, G, 0),
        [Op::MoveAbs { x: 960.0, y: 270.0 }]
    );
    assert_eq!(
        planner.plan(&InputMsg::MouseMove { x: -1.0, y: 2.0 }, G, 0),
        [Op::MoveAbs { x: 0.0, y: 1080.0 }]
    );
    assert!(planner
        .plan(
            &InputMsg::MouseMove {
                x: f32::NAN,
                y: 0.0,
            },
            G,
            0,
        )
        .is_empty());
    assert_eq!(planner.stats().dropped, 1);
}

#[test]
fn button_down_up_and_stray() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    for down in [true, false] {
        assert_eq!(
            planner.plan(&button(Button::Left, down), G, 0),
            [
                Op::MoveAbs { x: 0.0, y: 0.0 },
                Op::Button { code: 0x110, down }
            ]
        );
    }
    assert_eq!(
        planner.plan(&button(Button::Left, false), G, 0),
        [Op::MoveAbs { x: 0.0, y: 0.0 }]
    );
    assert_eq!(planner.stats().stray_up, 1);
    assert_eq!(
        planner.plan(&button(Button::Back, true), G, 0),
        [
            Op::MoveAbs { x: 0.0, y: 0.0 },
            Op::Button {
                code: 0x113,
                down: true,
            },
        ]
    );
}

#[test]
fn key_with_modifiers() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    assert_eq!(
        planner.plan(&key(0x04, true, Mods::LCTRL), G, 0),
        [
            Op::Key {
                code: 29,
                down: true
            },
            Op::Key {
                code: 30,
                down: true
            },
        ]
    );
    assert_eq!(
        planner.plan(&key(0x04, false, Mods::NONE), G, 0),
        [
            Op::Key {
                code: 29,
                down: false
            },
            Op::Key {
                code: 30,
                down: false
            },
        ]
    );
    assert!(planner.release_all().is_empty());
}

#[test]
fn modifier_key_itself() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    assert_eq!(
        planner.plan(&key(0xE1, true, Mods::NONE), G, 0),
        [Op::Key {
            code: 42,
            down: true
        }]
    );
    assert_eq!(
        planner.plan(&key(0xE1, false, Mods::LSHIFT), G, 0),
        [Op::Key {
            code: 42,
            down: false
        }]
    );
    assert!(planner.release_all().is_empty());
}

#[test]
fn unknown_and_overflow() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    assert!(planner.plan(&key(0x66, true, Mods::LCTRL), G, 0).is_empty());
    assert_eq!(planner.stats().unknown_usage, 1);
    assert!(planner.release_all().is_empty());
    assert_eq!(MAX_PRESSED, 32);
    for usage in 0x04..=0x24 {
        let ops = planner.plan(&key(usage, true, Mods::NONE), G, 0);
        assert_eq!(ops.len(), usize::from(usage != 0x24), "usage {usage:#x}");
    }
    assert_eq!(planner.stats().overflow, 1);
    assert_eq!(planner.release_all().len(), MAX_PRESSED);
    assert_eq!(planner.stats().unknown_usage, 1);
    assert_eq!(planner.stats().overflow, 1);
}

#[test]
fn autorepeat() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    for _ in 0..2 {
        assert_eq!(
            planner.plan(&key(0x04, true, Mods::NONE), G, 0),
            [Op::Key {
                code: 30,
                down: true
            }]
        );
    }
    assert_eq!(
        planner.plan(&key(0x04, false, Mods::NONE), G, 0),
        [Op::Key {
            code: 30,
            down: false
        }]
    );
    assert!(planner.plan(&key(0x04, false, Mods::NONE), G, 0).is_empty());
    assert_eq!(planner.stats().stray_up, 1);
    assert!(planner.release_all().is_empty());
}

#[test]
fn text_releases_and_restores_mods() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    planner.plan(&key(0xE1, true, Mods::NONE), G, 0);
    assert_eq!(
        planner.plan(&InputMsg::Text("aé\n中".into()), G, 0),
        [
            Op::Key {
                code: 42,
                down: false
            },
            Op::Keysym {
                sym: 0x61,
                down: true
            },
            Op::Keysym {
                sym: 0x61,
                down: false
            },
            Op::Keysym {
                sym: 0xE9,
                down: true
            },
            Op::Keysym {
                sym: 0xE9,
                down: false
            },
            Op::Key {
                code: 28,
                down: true
            },
            Op::Key {
                code: 28,
                down: false
            },
            Op::Keysym {
                sym: 0x0100_4E2D,
                down: true
            },
            Op::Keysym {
                sym: 0x0100_4E2D,
                down: false
            },
            Op::Key {
                code: 42,
                down: true
            },
        ]
    );
    assert_eq!(
        planner.plan(&key(0x04, true, Mods::LSHIFT), G, 0),
        [Op::Key {
            code: 30,
            down: true
        }]
    );
    assert_eq!(
        planner.release_all(),
        [
            Op::Key {
                code: 30,
                down: false
            },
            Op::Key {
                code: 42,
                down: false
            },
        ]
    );
    assert_eq!(planner.stats(), LinuxInputStats::default());
}

#[test]
fn text_tab_and_controls() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    assert_eq!(
        planner.plan(&InputMsg::Text("a\tb\u{1b}".into()), G, 0),
        [
            Op::Keysym {
                sym: 0x61,
                down: true
            },
            Op::Keysym {
                sym: 0x61,
                down: false
            },
            Op::Key {
                code: 15,
                down: true
            },
            Op::Key {
                code: 15,
                down: false
            },
            Op::Keysym {
                sym: 0x62,
                down: true
            },
            Op::Keysym {
                sym: 0x62,
                down: false
            },
        ]
    );
    assert_eq!(planner.stats().dropped, 1);
}

#[test]
fn portal_pixel_scroll_inverts() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    assert_eq!(
        planner.plan(&pixel(3.0, 10.0), G, 0),
        [Op::Axis {
            dx: -3.0,
            dy: -10.0
        }]
    );
    assert_eq!(planner.next_deadline(), None);
    assert!(planner.poll(u64::MAX).is_empty());
    assert!(planner.release_all().is_empty());
}

#[test]
fn portal_gesture_stop_after_timeout() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    for (phase, now) in [(ScrollPhase::Began, 0), (ScrollPhase::Changed, 100_000)] {
        assert_eq!(
            planner.plan(
                &scroll(0.0, 5.0, phase, ScrollPhase::None, ScrollUnit::Pixel),
                G,
                now
            ),
            [Op::Axis { dx: 0.0, dy: -5.0 }]
        );
        assert_eq!(planner.next_deadline(), Some(now + SCROLL_STOP_US));
    }
    assert!(planner
        .plan(
            &scroll(
                0.0,
                0.0,
                ScrollPhase::Ended,
                ScrollPhase::None,
                ScrollUnit::Pixel
            ),
            G,
            150_000,
        )
        .is_empty());
    assert_eq!(planner.next_deadline(), Some(450_000));
    assert!(planner.poll(400_000).is_empty());
    assert_eq!(planner.poll(450_000), [Op::AxisStop]);
    assert_eq!(planner.next_deadline(), None);
    assert!(planner.poll(1_000_000).is_empty());
}

#[test]
fn portal_momentum_end_stops_immediately() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    planner.plan(
        &scroll(
            0.0,
            5.0,
            ScrollPhase::Began,
            ScrollPhase::None,
            ScrollUnit::Pixel,
        ),
        G,
        0,
    );
    planner.plan(
        &scroll(
            0.0,
            0.0,
            ScrollPhase::Ended,
            ScrollPhase::None,
            ScrollUnit::Pixel,
        ),
        G,
        150_000,
    );
    assert_eq!(
        planner.plan(
            &scroll(
                0.0,
                8.0,
                ScrollPhase::None,
                ScrollPhase::Began,
                ScrollUnit::Pixel
            ),
            G,
            200_000,
        ),
        [Op::Axis { dx: 0.0, dy: -8.0 }]
    );
    assert_eq!(planner.next_deadline(), Some(500_000));
    assert_eq!(
        planner.plan(
            &scroll(
                0.0,
                0.0,
                ScrollPhase::None,
                ScrollPhase::Ended,
                ScrollUnit::Pixel
            ),
            G,
            250_000,
        ),
        [Op::AxisStop]
    );
    assert_eq!(planner.next_deadline(), None);
    assert!(planner.poll(1_000_000).is_empty());
    assert!(planner.release_all().is_empty());
}

#[test]
fn portal_line_scroll_uses_wheel() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    assert!(planner.plan(&line(0.0, 1.0), G, 0).is_empty());
    assert!(planner.plan(&line(0.0, 1.0), G, 0).is_empty());
    assert_eq!(
        planner.plan(&line(0.0, 1.0), G, 0),
        [Op::Wheel {
            axis: WheelAxis::Vertical,
            steps: -1
        }]
    );
    assert_eq!(
        planner.plan(&line(0.0, -3.0), G, 0),
        [Op::Wheel {
            axis: WheelAxis::Vertical,
            steps: 1
        }]
    );
    assert!(planner.plan(&line(2.0, 0.0), G, 0).is_empty());
    for _ in 0..2 {
        assert_eq!(
            planner.plan(&line(2.0, 0.0), G, 0),
            [Op::Wheel {
                axis: WheelAxis::Horizontal,
                steps: -1
            }]
        );
    }
    assert_eq!(planner.next_deadline(), None);
    assert!(planner.release_all().is_empty());
}

#[test]
fn x11_pixel_scroll() {
    let mut planner = LinuxPlanner::new(ScrollMode::X11);
    let expected = [Op::Wheel {
        axis: WheelAxis::Vertical,
        steps: -1,
    }];
    assert_eq!(planner.plan(&pixel(0.0, 100.0), G, 0), expected);
    assert!(planner.plan(&pixel(0.0, 50.0), G, 0).is_empty());
    assert_eq!(planner.plan(&pixel(0.0, 50.0), G, 0), expected);

    let phases = [
        ScrollPhase::None,
        ScrollPhase::Began,
        ScrollPhase::Changed,
        ScrollPhase::Ended,
    ];
    for unit in [ScrollUnit::Pixel, ScrollUnit::Line] {
        for phase in phases {
            for momentum in phases {
                let ops = planner.plan(&scroll(120.0, -120.0, phase, momentum, unit), G, 0);
                assert!(ops.iter().all(|op| matches!(op, Op::Wheel { .. })));
                assert_eq!(planner.next_deadline(), None);
            }
        }
    }
    assert!(planner.poll(u64::MAX).is_empty());
    assert!(planner.release_all().is_empty());
}

#[test]
fn began_resets_remainder() {
    let mut planner = LinuxPlanner::new(ScrollMode::X11);
    assert!(planner.plan(&pixel(0.0, 50.0), G, 0).is_empty());
    assert!(planner
        .plan(
            &scroll(
                0.0,
                50.0,
                ScrollPhase::Began,
                ScrollPhase::None,
                ScrollUnit::Pixel
            ),
            G,
            0,
        )
        .is_empty());
    assert_eq!(
        planner.plan(&pixel(0.0, 50.0), G, 0),
        [Op::Wheel {
            axis: WheelAxis::Vertical,
            steps: -1
        }]
    );
}

#[test]
fn release_all_order() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    planner.plan(&key(0x04, true, Mods::NONE), G, 0);
    planner.plan(&key(0x05, true, Mods::NONE), G, 0);
    planner.plan(&key(0xE0, true, Mods::NONE), G, 0);
    planner.plan(&key(0xE5, true, Mods::LCTRL), G, 0);
    planner.plan(&button(Button::Left, true), G, 0);
    planner.plan(&button(Button::Middle, true), G, 0);
    planner.plan(
        &scroll(
            0.0,
            5.0,
            ScrollPhase::Began,
            ScrollPhase::None,
            ScrollUnit::Pixel,
        ),
        G,
        0,
    );
    assert_eq!(
        planner.release_all(),
        [
            Op::Key {
                code: 48,
                down: false
            },
            Op::Key {
                code: 30,
                down: false
            },
            Op::Key {
                code: 54,
                down: false
            },
            Op::Key {
                code: 29,
                down: false
            },
            Op::Button {
                code: 0x110,
                down: false
            },
            Op::Button {
                code: 0x112,
                down: false
            },
            Op::AxisStop,
        ]
    );
    assert!(planner.release_all().is_empty());
    assert_eq!(planner.next_deadline(), None);
    assert!(planner.poll(u64::MAX).is_empty());
}

#[test]
fn release_all_message() {
    let mut direct = LinuxPlanner::new(ScrollMode::Portal);
    let mut message = LinuxPlanner::new(ScrollMode::Portal);
    for msg in [
        key(0x04, true, Mods::RALT),
        button(Button::Forward, true),
        scroll(
            1.0,
            2.0,
            ScrollPhase::Changed,
            ScrollPhase::None,
            ScrollUnit::Pixel,
        ),
    ] {
        assert_eq!(direct.plan(&msg, G, 0), message.plan(&msg, G, 0));
    }
    assert_eq!(
        message.plan(&InputMsg::ReleaseAll, G, 1),
        direct.release_all()
    );
    assert!(message.release_all().is_empty());
    assert_eq!(message.next_deadline(), None);
}

#[test]
fn touch_dropped() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    assert!(planner
        .plan(
            &InputMsg::Touch {
                id: 1,
                phase: TouchPhase::Began,
                x: 0.5,
                y: 0.5,
                pressure: 1.0,
            },
            G,
            0,
        )
        .is_empty());
    assert_eq!(planner.stats().dropped, 1);
    assert!(planner.release_all().is_empty());
}

#[test]
fn nonfinite_scroll_dropped() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    assert!(planner.plan(&pixel(f32::NAN, 0.0), G, 0).is_empty());
    assert_eq!(planner.stats().dropped, 1);
    assert_eq!(planner.next_deadline(), None);
}

#[test]
fn repeated_button_down_moves_each_time_and_releases_once() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    let msg = InputMsg::MouseButton {
        button: Button::Left,
        down: true,
        x: 0.5,
        y: 0.25,
        clicks: 2,
    };
    for _ in 0..2 {
        assert_eq!(
            planner.plan(&msg, G, 0),
            [
                Op::MoveAbs { x: 960.0, y: 270.0 },
                Op::Button {
                    code: 0x110,
                    down: true
                },
            ]
        );
    }
    assert_eq!(
        planner.release_all(),
        [Op::Button {
            code: 0x110,
            down: false
        }]
    );
    assert_eq!(planner.stats(), LinuxInputStats::default());
}

#[test]
fn nonfinite_coordinates_do_not_change_button_state() {
    for mode in [ScrollMode::Portal, ScrollMode::X11] {
        let mut planner = LinuxPlanner::new(mode);
        planner.plan(&button(Button::Left, true), G, 0);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for (x, y) in [(bad, 0.0), (0.0, bad)] {
                assert!(planner.plan(&InputMsg::MouseMove { x, y }, G, 0).is_empty());
                assert!(planner
                    .plan(
                        &InputMsg::MouseButton {
                            button: Button::Left,
                            down: false,
                            x,
                            y,
                            clicks: 1,
                        },
                        G,
                        0,
                    )
                    .is_empty());
            }
        }
        assert_eq!(planner.stats().dropped, 12);
        assert_eq!(planner.stats().stray_up, 0);
        assert_eq!(
            planner.release_all(),
            [Op::Button {
                code: 0x110,
                down: false
            }]
        );
    }
}

#[test]
fn small_displays_and_nonfinite_geometry() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    for (width, height) in [(1.0, 1.0), (0.0, 0.5), (-1.0, -2.0)] {
        assert_eq!(
            planner.plan(
                &InputMsg::MouseMove { x: 1.0, y: 1.0 },
                DisplayGeom { width, height },
                0
            ),
            [Op::MoveAbs { x: 0.0, y: 0.0 }]
        );
    }
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for (width, height) in [(bad, 1.0), (1.0, bad)] {
            assert!(planner
                .plan(
                    &InputMsg::MouseMove { x: 0.5, y: 0.5 },
                    DisplayGeom { width, height },
                    0
                )
                .is_empty());
        }
    }
    assert_eq!(planner.stats().dropped, 6);
}

#[test]
fn all_modifiers_and_buttons_release_in_documented_order() {
    let mut planner = LinuxPlanner::new(ScrollMode::X11);
    let codes = [29, 42, 56, 125, 97, 54, 100, 126];
    let mut expected = codes.map(|code| Op::Key { code, down: true }).to_vec();
    expected.push(Op::Key {
        code: 30,
        down: true,
    });
    assert_eq!(
        planner.plan(&key(0x04, true, Mods::from_bits(0xFF)), G, 0),
        expected
    );
    for button_id in [
        Button::Forward,
        Button::Back,
        Button::Middle,
        Button::Right,
        Button::Left,
    ] {
        planner.plan(&button(button_id, true), G, 0);
    }
    let mut expected = vec![Op::Key {
        code: 30,
        down: false,
    }];
    expected.extend(
        codes
            .into_iter()
            .rev()
            .map(|code| Op::Key { code, down: false }),
    );
    expected.extend((0x110..=0x114).map(|code| Op::Button { code, down: false }));
    assert_eq!(planner.release_all(), expected);
    assert!(planner.release_all().is_empty());
}

#[test]
fn overflow_and_stray_key_still_sync_modifiers() {
    let mut planner = LinuxPlanner::new(ScrollMode::X11);
    for usage in 0x04..=0x23 {
        planner.plan(&key(usage, true, Mods::NONE), G, 0);
    }
    assert_eq!(
        planner.plan(&key(0x24, true, Mods::RCTRL), G, 0),
        [Op::Key {
            code: 97,
            down: true
        }]
    );
    assert_eq!(planner.stats().overflow, 1);
    assert_eq!(
        planner.plan(&key(0x04, true, Mods::RCTRL), G, 0),
        [Op::Key {
            code: 30,
            down: true
        }]
    );
    assert_eq!(planner.stats().overflow, 1);
    assert_eq!(
        planner.plan(&key(0x24, false, Mods::NONE), G, 0),
        [Op::Key {
            code: 97,
            down: false
        }]
    );
    assert_eq!(planner.stats().stray_up, 1);
    planner.plan(&key(0x04, false, Mods::NONE), G, 0);
    assert_eq!(
        planner.plan(&key(0x24, true, Mods::NONE), G, 0),
        [Op::Key {
            code: 8,
            down: true
        }]
    );
}

#[test]
fn text_decodes_surrogates_and_restores_multiple_modifiers_in_order() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    planner.plan(&key(0x04, true, Mods::LCTRL | Mods::RSHIFT), G, 0);
    assert_eq!(
        planner.plan(&InputMsg::Text("😀\r\n\u{85}".into()), G, 0),
        [
            Op::Key {
                code: 54,
                down: false
            },
            Op::Key {
                code: 29,
                down: false
            },
            Op::Keysym {
                sym: 0x0101_F600,
                down: true
            },
            Op::Keysym {
                sym: 0x0101_F600,
                down: false
            },
            Op::Key {
                code: 28,
                down: true
            },
            Op::Key {
                code: 28,
                down: false
            },
            Op::Key {
                code: 29,
                down: true
            },
            Op::Key {
                code: 54,
                down: true
            },
        ]
    );
    assert_eq!(planner.stats().dropped, 1);
    assert_eq!(
        planner.release_all(),
        [
            Op::Key {
                code: 30,
                down: false
            },
            Op::Key {
                code: 54,
                down: false
            },
            Op::Key {
                code: 29,
                down: false
            },
        ]
    );
}

#[test]
fn wheel_axes_order_reversal_and_release_all_clear_remainders() {
    for mode in [ScrollMode::Portal, ScrollMode::X11] {
        let mut planner = LinuxPlanner::new(mode);
        assert_eq!(
            planner.plan(&line(6.0, -3.0), G, 0),
            [
                Op::Wheel {
                    axis: WheelAxis::Vertical,
                    steps: 1
                },
                Op::Wheel {
                    axis: WheelAxis::Horizontal,
                    steps: -2
                },
            ]
        );
        assert!(planner.plan(&line(2.0, -2.0), G, 0).is_empty());
        assert!(planner.plan(&line(-1.0, 1.0), G, 0).is_empty());
        assert!(planner.release_all().is_empty());
        assert!(planner.plan(&line(2.0, -2.0), G, 0).is_empty());
    }
}

#[test]
fn zero_gestures_refresh_deadline_and_end_delta_precedes_stop() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    assert!(planner.plan(&pixel(0.0, 0.0), G, 0).is_empty());
    assert_eq!(planner.next_deadline(), None);
    assert!(planner
        .plan(
            &scroll(
                0.0,
                0.0,
                ScrollPhase::Changed,
                ScrollPhase::None,
                ScrollUnit::Pixel
            ),
            G,
            0,
        )
        .is_empty());
    assert_eq!(planner.next_deadline(), Some(SCROLL_STOP_US));
    assert!(planner
        .plan(
            &scroll(
                0.0,
                0.0,
                ScrollPhase::None,
                ScrollPhase::Changed,
                ScrollUnit::Pixel
            ),
            G,
            100_000,
        )
        .is_empty());
    assert!(planner.poll(399_999).is_empty());
    assert_eq!(planner.next_deadline(), Some(400_000));
    assert_eq!(
        planner.plan(
            &scroll(
                -2.0,
                4.0,
                ScrollPhase::None,
                ScrollPhase::Ended,
                ScrollUnit::Pixel
            ),
            G,
            200_000,
        ),
        [Op::Axis { dx: 2.0, dy: -4.0 }, Op::AxisStop]
    );
    assert!(planner.poll(u64::MAX).is_empty());
}

#[test]
fn nonfinite_scroll_does_not_reset_remainders_or_deadline() {
    for mode in [ScrollMode::Portal, ScrollMode::X11] {
        let mut planner = LinuxPlanner::new(mode);
        planner.plan(&line(2.0, 2.0), G, 0);
        planner.plan(
            &scroll(
                0.0,
                0.0,
                ScrollPhase::Began,
                ScrollPhase::None,
                ScrollUnit::Pixel,
            ),
            G,
            0,
        );
        // A valid Begin clears both remainders; seed them again before invalid events.
        planner.plan(&line(2.0, 2.0), G, 0);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for (dx, dy) in [(bad, 0.0), (0.0, bad)] {
                assert!(planner
                    .plan(
                        &scroll(
                            dx,
                            dy,
                            ScrollPhase::Began,
                            ScrollPhase::Ended,
                            ScrollUnit::Pixel
                        ),
                        G,
                        100_000,
                    )
                    .is_empty());
            }
        }
        assert_eq!(planner.stats().dropped, 6);
        assert_eq!(
            planner.next_deadline(),
            if mode == ScrollMode::Portal {
                Some(SCROLL_STOP_US)
            } else {
                None
            }
        );
        assert_eq!(
            planner.plan(&line(1.0, 1.0), G, 100_000),
            [
                Op::Wheel {
                    axis: WheelAxis::Vertical,
                    steps: -1
                },
                Op::Wheel {
                    axis: WheelAxis::Horizontal,
                    steps: -1
                },
            ]
        );
    }
}

#[test]
fn portal_line_gestures_never_start_smooth_scroll() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    for (phase, momentum) in [
        (ScrollPhase::Began, ScrollPhase::None),
        (ScrollPhase::Ended, ScrollPhase::None),
        (ScrollPhase::None, ScrollPhase::Began),
        (ScrollPhase::None, ScrollPhase::Ended),
    ] {
        assert_eq!(
            planner.plan(&scroll(0.0, 3.0, phase, momentum, ScrollUnit::Line), G, 0),
            [Op::Wheel {
                axis: WheelAxis::Vertical,
                steps: -1
            }]
        );
        assert_eq!(planner.next_deadline(), None);
    }
    assert!(planner.poll(u64::MAX).is_empty());
    assert!(planner.release_all().is_empty());
}

#[test]
fn deadline_saturates_and_extreme_wheel_delta_does_not_leave_backlog() {
    let mut planner = LinuxPlanner::new(ScrollMode::Portal);
    planner.plan(
        &scroll(
            0.0,
            0.0,
            ScrollPhase::Began,
            ScrollPhase::None,
            ScrollUnit::Pixel,
        ),
        G,
        u64::MAX - 1,
    );
    assert_eq!(planner.next_deadline(), Some(u64::MAX));
    assert!(planner.poll(u64::MAX - 1).is_empty());
    assert_eq!(planner.poll(u64::MAX), [Op::AxisStop]);

    let mut planner = LinuxPlanner::new(ScrollMode::X11);
    assert_eq!(
        planner.plan(&pixel(f32::MAX, -f32::MAX), G, 0),
        [
            Op::Wheel {
                axis: WheelAxis::Vertical,
                steps: i32::MAX
            },
            Op::Wheel {
                axis: WheelAxis::Horizontal,
                steps: i32::MIN
            },
        ]
    );
    assert!(planner.plan(&pixel(0.0, 0.0), G, 0).is_empty());
}
