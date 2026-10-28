use host_core::inject::win_plan::{HWHEEL_SIGN, WHEEL_UNITS_PER_LINE};
use rs_desktop::platform::ScrollInput;
use rs_desktop::scroll::{mac_phase, ScrollMapper, WIN_WHEEL_UNITS_PER_LINE};
use rsp_core::hid::{InputMsg, ScrollPhase, ScrollUnit};

fn precise(dx: f64, dy: f64, phase: u64, momentum: u64) -> ScrollInput {
    ScrollInput::MacPrecise {
        dx,
        dy,
        phase,
        momentum,
    }
}

#[test]
fn mac_phase_bits() {
    assert_eq!(mac_phase(0), Some(ScrollPhase::None));
    assert_eq!(mac_phase(1), Some(ScrollPhase::Began));
    assert_eq!(mac_phase(4), Some(ScrollPhase::Changed));
    assert_eq!(mac_phase(2), Some(ScrollPhase::Changed));
    assert_eq!(mac_phase(8), Some(ScrollPhase::Ended));
    assert_eq!(mac_phase(16), Some(ScrollPhase::Ended));
    assert_eq!(mac_phase(32), None);
}

#[test]
fn mac_precise_gesture_and_momentum() {
    let mut mapper = ScrollMapper::new();
    assert_eq!(
        mapper.map(precise(0.0, 3.5, 4, 0)),
        Some(InputMsg::Scroll {
            dx: 0.0,
            dy: 3.5,
            phase: ScrollPhase::Changed,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Pixel,
        })
    );
    assert_eq!(
        mapper.map(precise(0.0, 2.0, 0, 4)),
        Some(InputMsg::Scroll {
            dx: 0.0,
            dy: 2.0,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::Changed,
            unit: ScrollUnit::Pixel,
        })
    );
}

#[test]
fn mac_both_phases_and_may_begin_dropped() {
    let mut mapper = ScrollMapper::new();
    assert_eq!(mapper.map(precise(1.0, 1.0, 4, 4)), None);
    assert_eq!(mapper.map(precise(1.0, 1.0, 32, 0)), None);
}

#[test]
fn mac_zero_rules() {
    let mut mapper = ScrollMapper::new();
    assert_eq!(mapper.map(precise(0.0, 0.0, 0, 0)), None);
    assert_eq!(
        mapper.map(precise(0.0, 0.0, 1, 0)),
        Some(InputMsg::Scroll {
            dx: 0.0,
            dy: 0.0,
            phase: ScrollPhase::Began,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Pixel,
        })
    );
    assert_eq!(mapper.map(precise(0.0, 0.0, 2, 0)), None);
}

#[test]
fn mac_lines() {
    let mut mapper = ScrollMapper::new();
    assert_eq!(
        mapper.map(ScrollInput::MacLines { dx: 0.0, dy: -3.0 }),
        Some(InputMsg::Scroll {
            dx: 0.0,
            dy: -3.0,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Line,
        })
    );
}

#[test]
fn win_wheel_directions() {
    let mut mapper = ScrollMapper::new();
    assert_eq!(
        mapper.map(ScrollInput::WinWheel {
            delta: 120,
            horizontal: false,
        }),
        Some(InputMsg::Scroll {
            dx: 0.0,
            dy: 3.0,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Line,
        })
    );
    assert_eq!(
        mapper.map(ScrollInput::WinWheel {
            delta: -240,
            horizontal: false,
        }),
        Some(InputMsg::Scroll {
            dx: 0.0,
            dy: -6.0,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Line,
        })
    );
    assert_eq!(
        mapper.map(ScrollInput::WinWheel {
            delta: 120,
            horizontal: true,
        }),
        Some(InputMsg::Scroll {
            dx: -3.0,
            dy: 0.0,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Line,
        })
    );
}

#[test]
fn win_roundtrip_against_host() {
    assert_eq!(WIN_WHEEL_UNITS_PER_LINE, WHEEL_UNITS_PER_LINE);
    let mut mapper = ScrollMapper::new();
    for delta in [120, -120, 30] {
        let vertical = mapper
            .map(ScrollInput::WinWheel {
                delta,
                horizontal: false,
            })
            .expect("non-zero wheel");
        let horizontal = mapper
            .map(ScrollInput::WinWheel {
                delta,
                horizontal: true,
            })
            .expect("non-zero wheel");
        let InputMsg::Scroll { dx, dy, .. } = vertical else {
            panic!("vertical wheel must be scroll");
        };
        assert!((f64::from(dy) * WHEEL_UNITS_PER_LINE - f64::from(delta)).abs() < 1e-5);
        assert_eq!(dx, 0.0);
        let InputMsg::Scroll { dx, dy, .. } = horizontal else {
            panic!("horizontal wheel must be scroll");
        };
        assert!(
            (f64::from(dx) * WHEEL_UNITS_PER_LINE * f64::from(HWHEEL_SIGN) - f64::from(delta))
                .abs()
                < 1e-5
        );
        assert_eq!(dy, 0.0);
    }
}

#[test]
fn non_finite_dropped() {
    let mut mapper = ScrollMapper::new();
    assert_eq!(mapper.map(precise(f64::NAN, 1.0, 0, 0)), None);
    assert_eq!(
        mapper.map(ScrollInput::MacLines {
            dx: 1.0,
            dy: f64::INFINITY
        }),
        None
    );
}
