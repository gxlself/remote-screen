use host_core::platform::DisplayInfo;
use host_core::vdisplay::arrange::{mac_rect, place_beside, DeskRect};
use host_core::vdisplay::mode::{choose, physical_size_mm, points, ModeError, VdRequest};
use host_core::vdisplay::{VdCaps, VdMode};

fn mac() -> VdCaps {
    VdCaps {
        max_long_px: 7680,
        max_short_px: 4320,
        min_long_px: 640,
        min_short_px: 480,
        refresh_rates: vec![30, 60],
        scales: vec![1.0, 2.0],
        align_px: 4,
        max_displays: 2,
    }
}

fn win() -> VdCaps {
    VdCaps {
        max_long_px: 3840,
        max_short_px: 2560,
        min_long_px: 640,
        min_short_px: 480,
        refresh_rates: vec![30, 60],
        scales: vec![1.0, 1.5, 2.0],
        align_px: 8,
        max_displays: 4,
    }
}

fn req(width_px: u32, height_px: u32, scale: f32, refresh_hz: u32) -> VdRequest {
    VdRequest {
        width_px,
        height_px,
        scale,
        refresh_hz,
    }
}

#[test]
fn ipad_on_mac() {
    assert_eq!(
        choose(&req(2732, 2048, 0.0, 0), &mac()),
        Ok(VdMode {
            width_px: 2732,
            height_px: 2048,
            scale: 2.0,
            refresh_hz: 60,
        })
    );
}

#[test]
fn ipad_on_windows() {
    assert_eq!(
        choose(&req(2732, 2048, 0.0, 0), &win()),
        Ok(VdMode {
            width_px: 2728,
            height_px: 2048,
            scale: 2.0,
            refresh_hz: 60,
        })
    );
}

#[test]
fn portrait_kept() {
    let mode = choose(&req(1179, 2556, 2.0, 60), &mac()).expect("mode should fit");
    assert_eq!(mode.width_px, 1176);
    assert_eq!(mode.height_px, 2556);
    assert_eq!(mode.scale, 2.0);
}

#[test]
fn scale_default_small_short_side() {
    let mode = choose(&req(1179, 2556, 0.0, 0), &mac()).expect("mode should fit");
    assert_eq!(mode.scale, 1.0);
}

#[test]
fn shrinks_proportionally() {
    let landscape = choose(&req(8000, 5000, 1.0, 60), &mac()).expect("mode should fit");
    assert_eq!((landscape.width_px, landscape.height_px), (6912, 4320));

    let portrait = choose(&req(5000, 8000, 1.0, 60), &mac()).expect("mode should fit");
    assert_eq!((portrait.width_px, portrait.height_px), (4320, 6912));
}

#[test]
fn shrinks_on_windows() {
    let mode = choose(&req(5120, 2880, 1.0, 60), &win()).expect("mode should fit");
    assert_eq!((mode.width_px, mode.height_px), (3840, 2160));
}

#[test]
fn missing_size() {
    assert_eq!(
        choose(&req(0, 100, 1.0, 60), &mac()),
        Err(ModeError::MissingSize)
    );
    assert_eq!(
        choose(&req(100, 0, 1.0, 60), &mac()),
        Err(ModeError::MissingSize)
    );
}

#[test]
fn too_small() {
    assert_eq!(
        choose(&req(100, 100, 1.0, 60), &mac()),
        Err(ModeError::TooSmall {
            width: 100,
            height: 100,
        })
    );
    assert_eq!(
        choose(&req(640, 479, 1.0, 60), &mac()),
        Err(ModeError::TooSmall {
            width: 640,
            height: 479,
        })
    );
}

#[test]
fn scale_nearest_and_tie() {
    for (requested, expected) in [(1.4, 1.0), (1.5, 1.0), (1.6, 2.0), (3.0, 2.0)] {
        let mode = choose(&req(2048, 1536, requested, 60), &mac()).expect("mode should fit");
        assert_eq!(mode.scale, expected);
    }
    for (requested, expected) in [(1.74, 1.5), (1.76, 2.0)] {
        let mode = choose(&req(2048, 1536, requested, 60), &win()).expect("mode should fit");
        assert_eq!(mode.scale, expected);
    }

    for requested in [f32::NAN, -1.0] {
        let mode = choose(&req(2732, 2048, requested, 60), &mac()).expect("mode should fit");
        assert_eq!(mode.scale, 2.0);
    }
}

#[test]
fn refresh_pick() {
    for (requested, expected) in [(120, 60), (45, 30), (20, 30), (0, 60)] {
        let mode = choose(&req(1920, 1080, 1.0, requested), &mac()).expect("mode should fit");
        assert_eq!(mode.refresh_hz, expected);
    }

    let mut caps = mac();
    caps.refresh_rates.clear();
    let mode = choose(&req(1920, 1080, 1.0, 60), &caps).expect("mode should fit");
    assert_eq!(mode.refresh_hz, 60);
}

#[test]
fn align_floor_two() {
    for align_px in [0, 1] {
        let mut caps = mac();
        caps.align_px = align_px;
        let mode = choose(&req(1001, 801, 1.0, 60), &caps).expect("mode should fit");
        assert_eq!((mode.width_px, mode.height_px), (1000, 800));
    }
}

#[test]
fn empty_scales() {
    let mut caps = mac();
    caps.scales.clear();
    let mode = choose(&req(1920, 1080, 0.0, 60), &caps).expect("mode should fit");
    assert_eq!(mode.scale, 1.0);

    caps.scales = vec![1.25];
    let mode = choose(&req(1920, 1080, 0.0, 60), &caps).expect("mode should fit");
    assert_eq!(mode.scale, 1.25);
}

#[test]
fn physical_size() {
    assert_eq!(
        physical_size_mm(&VdMode {
            width_px: 2732,
            height_px: 2048,
            scale: 2.0,
            refresh_hz: 60,
        }),
        (361, 271)
    );
    assert_eq!(
        physical_size_mm(&VdMode {
            width_px: 1920,
            height_px: 1080,
            scale: 1.0,
            refresh_hz: 60,
        }),
        (508, 286)
    );
    assert_eq!(
        physical_size_mm(&VdMode {
            width_px: 1920,
            height_px: 1080,
            scale: 0.5,
            refresh_hz: 60,
        }),
        (508, 286)
    );
}

#[test]
fn points_of() {
    assert_eq!(
        points(&VdMode {
            width_px: 2732,
            height_px: 2048,
            scale: 2.0,
            refresh_hz: 60,
        }),
        (1366, 1024)
    );
    assert_eq!(
        points(&VdMode {
            width_px: 2732,
            height_px: 2048,
            scale: 0.0,
            refresh_hz: 60,
        }),
        (2732, 2048)
    );
}

fn display(width_px: u32, height_px: u32, scale: f32, x: i32, y: i32) -> DisplayInfo {
    DisplayInfo {
        id: 1,
        name: "Display".into(),
        width_px,
        height_px,
        scale,
        x,
        y,
        refresh_hz: 60,
        primary: true,
        rotation: 0,
    }
}

#[test]
fn mac_rect_points() {
    assert_eq!(
        mac_rect(&display(3024, 1964, 2.0, 0, 0)),
        DeskRect {
            x: 0,
            y: 0,
            w: 1512,
            h: 982,
        }
    );
    assert_eq!(
        mac_rect(&display(3024, 1964, 0.0, -12, 7)),
        DeskRect {
            x: -12,
            y: 7,
            w: 3024,
            h: 1964,
        }
    );
}

#[test]
fn place_right_of_all() {
    let existing = [
        (
            DeskRect {
                x: 0,
                y: 0,
                w: 1512,
                h: 982,
            },
            true,
        ),
        (
            DeskRect {
                x: 1512,
                y: -200,
                w: 1920,
                h: 1080,
            },
            false,
        ),
    ];
    assert_eq!(place_beside(&existing), (3432, 0));

    let with_left = [
        (
            DeskRect {
                x: 0,
                y: 0,
                w: 1512,
                h: 982,
            },
            true,
        ),
        (
            DeskRect {
                x: -1920,
                y: 0,
                w: 1920,
                h: 1080,
            },
            false,
        ),
    ];
    assert_eq!(place_beside(&with_left), (1512, 0));
}

#[test]
fn place_empty_and_no_primary() {
    assert_eq!(place_beside(&[]), (0, 0));
    assert_eq!(
        place_beside(&[(
            DeskRect {
                x: 100,
                y: 50,
                w: 800,
                h: 600,
            },
            false,
        )]),
        (900, 0)
    );
    assert_eq!(
        place_beside(&[(
            DeskRect {
                x: 0,
                y: -30,
                w: 1512,
                h: 982,
            },
            true,
        )]),
        (1512, -30)
    );
}

#[test]
fn place_saturates() {
    assert_eq!(
        place_beside(&[(
            DeskRect {
                x: i32::MAX - 10,
                y: 0,
                w: 100,
                h: 100,
            },
            true,
        )]),
        (i32::MAX, 0)
    );
}
