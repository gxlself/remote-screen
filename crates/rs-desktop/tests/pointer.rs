use rs_desktop::platform::{PointerCapture, PointerInput};
use rs_desktop::pointer::{
    ClickCounter, PointerConfig, PointerOut, PointerState, PointerThrottle, Viewport,
};
use rsp_core::hid::{Button, InputMsg};

fn vp() -> Viewport {
    Viewport {
        view_w: 1000.0,
        view_h: 500.0,
        video_w: 1920,
        video_h: 1080,
    }
}

fn b(button: u8, down: bool, x: f64, y: f64, clicks: u8) -> PointerInput {
    PointerInput::Button {
        button,
        down,
        x,
        y,
        clicks,
    }
}

#[test]
fn content_rect_letterbox() {
    let (x, y, w, h) = vp().content_rect();
    assert!((x - 55.555_555_555_555_56).abs() < 1e-6);
    assert!(y.abs() < 1e-6);
    assert!((w - 888.888_888_888_888_8).abs() < 1e-6);
    assert!((h - 500.0).abs() < 1e-6);
    let portrait = Viewport {
        view_w: 500.0,
        view_h: 1000.0,
        ..vp()
    };
    let (x, y, w, h) = portrait.content_rect();
    assert!(x.abs() < 1e-6 && y > 0.0);
    assert!((w - 500.0).abs() < 1e-6 && (h - 281.25).abs() < 1e-6);
}

#[test]
fn normalized_inside_outside() {
    let view = vp();
    assert_eq!(view.to_normalized(500.0, 250.0), Some((0.5, 0.5)));
    assert_eq!(view.to_normalized(0.0, 250.0), None);
    let (x, y) = view.to_normalized_clamped(0.0, 250.0);
    assert_eq!(x, 0.0);
    assert!((y - 0.5).abs() < 1e-6);
}

#[test]
fn to_view_roundtrip() {
    let view = vp();
    let (left, top, width, height) = view.content_rect();
    let mut seed = 0x1234_5678_u64;
    for _ in 0..100 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let nx = seed as f64 / u64::MAX as f64;
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let ny = seed as f64 / u64::MAX as f64;
        let x = left + nx * width;
        let y = top + ny * height;
        let Some((got_x, got_y)) = view.to_normalized(x, y) else {
            panic!("generated point must be in content");
        };
        let (round_x, round_y) = view.to_view(got_x, got_y);
        assert!((round_x - x).abs() < 1e-3);
        assert!((round_y - y).abs() < 1e-3);
    }
}

#[test]
fn throttle_latest_wins() {
    let mut throttle = PointerThrottle::new(8_333);
    assert_eq!(throttle.push(0.1, 0.1, 0), Some((0.1, 0.1)));
    assert_eq!(throttle.push(0.2, 0.2, 1_000), None);
    assert_eq!(throttle.push(0.3, 0.3, 2_000), None);
    assert_eq!(throttle.flush(5_000), None);
    assert_eq!(throttle.flush(8_333), Some((0.3, 0.3)));
    assert_eq!(throttle.flush(9_000), None);
}

#[test]
fn button_carries_coords_and_drops_pending() {
    let mut state = PointerState::new(PointerConfig::default());
    state.set_viewport(vp());
    state.on_input(
        PointerInput::Move {
            x: 500.0,
            y: 250.0,
            dx: 0.0,
            dy: 0.0,
        },
        0,
    );
    assert!(state
        .on_input(
            PointerInput::Move {
                x: 510.0,
                y: 250.0,
                dx: 0.0,
                dy: 0.0,
            },
            1_000,
        )
        .is_empty());
    assert_eq!(
        state.on_input(b(0, true, 510.0, 250.0, 0), 2_000),
        vec![PointerOut::Input(InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.511_25,
            y: 0.5,
            clicks: 1,
        })]
    );
    assert!(state.flush(8_333).is_empty());
}

#[test]
fn double_click_counter() {
    let mut counter = ClickCounter::new(500_000, 4.0);
    assert_eq!(counter.down(0, 10.0, 10.0, 0), 1);
    assert_eq!(counter.down(0, 11.0, 10.0, 200_000), 2);
    assert_eq!(counter.down(0, 11.0, 10.0, 400_000), 3);
    assert_eq!(counter.down(0, 11.0, 10.0, 600_000), 1);
    assert_eq!(counter.down(0, 11.0, 10.0, 1_200_000), 1);
    assert_eq!(counter.down(0, 21.0, 10.0, 1_300_000), 1);
    assert_eq!(counter.down(1, 21.0, 10.0, 1_400_000), 1);
}

#[test]
fn pointer_click_slop_uses_view_points() {
    let mut state = PointerState::new(PointerConfig::default());
    state.set_viewport(Viewport {
        view_w: 1_000.0,
        view_h: 1_000.0,
        video_w: 1_000,
        video_h: 1_000,
    });
    state.on_input(b(0, true, 100.0, 100.0, 0), 0);
    state.on_input(b(0, false, 100.0, 100.0, 0), 1);
    let out = state.on_input(b(0, true, 110.0, 100.0, 0), 200_000);
    assert_eq!(
        out,
        vec![PointerOut::Input(InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.11,
            y: 0.1,
            clicks: 1,
        })]
    );
}

#[test]
fn platform_clicks_preferred() {
    let mut state = PointerState::new(PointerConfig::default());
    state.set_viewport(Viewport {
        view_w: 100.0,
        view_h: 100.0,
        video_w: 100,
        video_h: 100,
    });
    assert_eq!(
        state.on_input(b(0, true, 50.0, 50.0, 2), 0),
        vec![PointerOut::Input(InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.5,
            y: 0.5,
            clicks: 2,
        })]
    );
}

#[test]
fn drag_outside_clamps() {
    let mut state = PointerState::new(PointerConfig::default());
    state.set_viewport(vp());
    state.on_input(b(0, true, 500.0, 250.0, 0), 0);
    assert_eq!(
        state.on_input(
            PointerInput::Move {
                x: 0.0,
                y: 250.0,
                dx: 0.0,
                dy: 0.0,
            },
            8_333,
        ),
        vec![PointerOut::Pointer { x: 0.0, y: 0.5 }]
    );
    assert_eq!(
        state.on_input(b(0, false, 0.0, 250.0, 0), 9_000),
        vec![PointerOut::Input(InputMsg::MouseButton {
            button: Button::Left,
            down: false,
            x: 0.0,
            y: 0.5,
            clicks: 1,
        })]
    );
}

#[test]
fn down_outside_and_out_of_range_ignored() {
    let mut state = PointerState::new(PointerConfig::default());
    state.set_viewport(vp());
    assert!(state.on_input(b(0, true, 0.0, 250.0, 0), 0).is_empty());
    assert!(state.on_input(b(0, false, 0.0, 250.0, 1), 1).is_empty());
    assert!(state.on_input(b(7, true, 500.0, 250.0, 1), 2).is_empty());
}

#[test]
fn virtual_accumulates_and_uses_virtual_button_position() {
    let mut state = PointerState::new(PointerConfig::default());
    state.set_viewport(Viewport {
        view_w: 1_000.0,
        view_h: 1_000.0,
        video_w: 1_000,
        video_h: 1_000,
    });
    assert_eq!(
        state.set_capture(PointerCapture::Virtual),
        vec![PointerOut::VirtualCursor { x: 0.5, y: 0.5 }]
    );
    assert_eq!(
        state.on_input(
            PointerInput::Move {
                x: 999.0,
                y: 999.0,
                dx: 100.0,
                dy: -50.0,
            },
            0,
        ),
        vec![
            PointerOut::Pointer { x: 0.6, y: 0.45 },
            PointerOut::VirtualCursor { x: 0.6, y: 0.45 },
        ]
    );
    assert!(state
        .on_input(
            PointerInput::Button {
                button: 0,
                down: true,
                x: 9999.0,
                y: 9999.0,
                clicks: 0,
            },
            1,
        )
        .contains(&PointerOut::Input(InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.6,
            y: 0.45,
            clicks: 1,
        })));
    assert!(state
        .on_input(
            PointerInput::Move {
                x: 0.0,
                y: 0.0,
                dx: 10_000.0,
                dy: 10_000.0,
            },
            2,
        )
        .is_empty());
    assert_eq!(
        state.flush(8_333),
        vec![
            PointerOut::Pointer { x: 1.0, y: 1.0 },
            PointerOut::VirtualCursor { x: 1.0, y: 1.0 },
        ]
    );
}

#[test]
fn virtual_seeds_from_host() {
    let mut state = PointerState::new(PointerConfig::default());
    state.on_host_pointer(0.2, 0.3);
    assert_eq!(
        state.set_capture(PointerCapture::Virtual),
        vec![PointerOut::VirtualCursor { x: 0.2, y: 0.3 }]
    );
}

#[test]
fn release_all_reports_held_and_leave_clears_pending() {
    let mut state = PointerState::new(PointerConfig::default());
    state.set_viewport(Viewport {
        view_w: 100.0,
        view_h: 100.0,
        video_w: 100,
        video_h: 100,
    });
    state.on_input(b(0, true, 50.0, 50.0, 0), 0);
    assert!(state.release_all());
    assert_eq!(state.held_buttons(), 0);
    assert!(state.on_input(b(0, false, 50.0, 50.0, 0), 1).is_empty());
    assert!(!state.release_all());

    state.on_input(
        PointerInput::Move {
            x: 50.0,
            y: 50.0,
            dx: 0.0,
            dy: 0.0,
        },
        2,
    );
    state.on_input(
        PointerInput::Move {
            x: 51.0,
            y: 50.0,
            dx: 0.0,
            dy: 0.0,
        },
        3,
    );
    state.on_input(PointerInput::Leave, 4);
    assert!(state.flush(8_333).is_empty());
}

#[test]
fn viewport_roundtrip_with_unknown_video_size() {
    let view = Viewport {
        view_w: 100.0,
        view_h: 80.0,
        video_w: 0,
        video_h: 0,
    };
    assert_eq!(view.content_rect(), (0.0, 0.0, 100.0, 80.0));
    assert_eq!(view.to_normalized(25.0, 40.0), Some((0.25, 0.5)));
}
