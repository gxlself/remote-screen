#![cfg(target_os = "macos")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use host_core::platform::DisplayInfo;
use host_core::vdisplay::mode::{self, VdRequest};
use host_core::vdisplay::VdMode;
use host_mac::vdisplay;

#[test]
fn mac_caps_values() {
    let caps = vdisplay::mac_caps();
    assert_eq!(caps.min_long_px, 640);
    assert_eq!(caps.max_long_px, 7680);
    assert_eq!(caps.min_short_px, 480);
    assert_eq!(caps.max_short_px, 4320);
    assert_eq!(caps.refresh_rates, vec![30, 60]);
    assert_eq!(caps.scales, vec![1.0, 2.0]);
    assert_eq!(caps.align_px, 4);
    assert_eq!(caps.max_displays, 2);
}

#[test]
fn backend_is_singleton() {
    match (vdisplay::backend(), vdisplay::backend()) {
        (None, None) => {}
        (Some(first), Some(second)) => {
            assert!(Arc::ptr_eq(&first, &second));
            assert_eq!(first.caps(), Some(vdisplay::mac_caps()));
        }
        _ => panic!("virtual display availability changed between calls"),
    }
}

#[test]
fn supported_probe_is_cheap() {
    let started = Instant::now();
    let mut first = None;
    for _ in 0..100 {
        let supported = vdisplay::supported();
        match first {
            Some(expected) => assert_eq!(supported, expected),
            None => first = Some(supported),
        }
    }
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(1), "probe took {elapsed:?}");
}

#[test]
fn hidpi_rule() {
    for (scale, expected) in [(2.0, true), (1.5, true), (1.0, false), (1.49, false)] {
        assert_eq!(
            vdisplay::is_hidpi(&VdMode {
                width_px: 2732,
                height_px: 2048,
                scale,
                refresh_hz: 60,
            }),
            expected,
            "scale {scale}"
        );
    }
}

#[test]
fn destroy_unknown_token_ok() {
    if let Some(backend) = vdisplay::backend() {
        backend.destroy(12345).expect("unknown token is idempotent");
        assert_eq!(backend.cleanup_stale(), 0);
    }
}

#[test]
fn caps_choose_ipad() {
    let chosen = mode::choose(
        &VdRequest {
            width_px: 2732,
            height_px: 2048,
            scale: 0.0,
            refresh_hz: 0,
        },
        &vdisplay::mac_caps(),
    )
    .expect("iPad mode is supported");
    assert_eq!(
        chosen,
        VdMode {
            width_px: 2732,
            height_px: 2048,
            scale: 2.0,
            refresh_hz: 60,
        }
    );
}

fn system_mode(
    mode_id: u32,
    width_px: u32,
    height_px: u32,
    width_pt: u32,
    height_pt: u32,
    refresh_hz: f64,
) -> vdisplay::DisplayMode {
    vdisplay::DisplayMode {
        mode_id,
        width_px,
        height_px,
        width_pt,
        height_pt,
        usable: true,
        refresh_hz,
    }
}

#[test]
fn ipad_selects_backing_pixels_and_points_together() {
    let requested = VdMode {
        width_px: 2732,
        height_px: 2048,
        scale: 2.0,
        refresh_hz: 60,
    };
    let low_res = system_mode(1, 1366, 1024, 1366, 1024, 60.0);
    let native = system_mode(2, 2732, 2048, 2732, 2048, 60.0);
    let hidpi = system_mode(3, 2732, 2048, 1366, 1024, 60.0);
    assert_eq!(vdisplay::closest_mode(&[low_res, native], &requested), None);
    assert_eq!(
        vdisplay::closest_mode(&[low_res, native, hidpi], &requested),
        Some(hidpi)
    );
    assert_eq!(hidpi.actual(), Some(requested));
    assert_eq!(low_res.actual().expect("valid low-res mode").scale, 1.0);
    assert_eq!(
        vdisplay::closest_mode(
            &[hidpi, native],
            &VdMode {
                scale: 1.0,
                ..requested
            }
        ),
        Some(native)
    );
}

#[test]
fn refresh_uses_exact_mode_before_system_fallback() {
    let requested = VdMode {
        width_px: 1920,
        height_px: 1080,
        scale: 1.0,
        refresh_hz: 30,
    };
    let sixty = system_mode(1, 1920, 1080, 1920, 1080, 60.0);
    let thirty = system_mode(2, 1920, 1080, 1920, 1080, 30.0);
    // A 30 Hz HiDPI timing is not a usable substitute for the 1x request.
    let other_scale = system_mode(3, 1920, 1080, 960, 540, 30.0);
    assert_eq!(
        vdisplay::closest_mode(&[sixty, other_scale, thirty], &requested),
        Some(thirty)
    );
    let fallback = vdisplay::closest_mode(&[sixty, other_scale], &requested)
        .expect("60 Hz is the only available 1x timing");
    assert_eq!(fallback.actual().expect("known timing").refresh_hz, 60);
    assert_eq!(
        vdisplay::closest_mode(
            &[sixty, thirty],
            &VdMode {
                refresh_hz: 45,
                ..requested
            }
        ),
        Some(thirty),
        "equal-distance ties use the lower rate"
    );
}

#[test]
fn portrait_and_fractional_refresh_are_measured() {
    let requested = VdMode {
        width_px: 2048,
        height_px: 2732,
        scale: 2.0,
        refresh_hz: 60,
    };
    let portrait = system_mode(1, 2048, 2732, 1024, 1366, 59.94);
    assert_eq!(
        vdisplay::closest_mode(&[portrait], &requested),
        Some(portrait)
    );
    assert_eq!(portrait.actual(), Some(requested));
}

#[test]
fn unknown_or_invalid_modes_do_not_invent_actual_values() {
    let requested = VdMode {
        width_px: 1920,
        height_px: 1080,
        scale: 1.0,
        refresh_hz: 30,
    };
    for refresh_hz in [0.0, -30.0, f64::NAN, f64::INFINITY] {
        let unknown = system_mode(1, 1920, 1080, 1920, 1080, refresh_hz);
        assert_eq!(unknown.actual(), None);
        assert_eq!(vdisplay::closest_mode(&[unknown], &requested), None);
    }
    assert_eq!(system_mode(1, 0, 1080, 1920, 1080, 60.0).actual(), None);
    assert_eq!(system_mode(1, 1920, 1080, 0, 1080, 60.0).actual(), None);
    assert_eq!(system_mode(1, 1920, 1080, 960, 1080, 60.0).actual(), None);
    let unavailable = vdisplay::DisplayMode {
        usable: false,
        ..system_mode(1, 1920, 1080, 1920, 1080, 30.0)
    };
    assert_eq!(vdisplay::closest_mode(&[unavailable], &requested), None);
    assert_eq!(
        vdisplay::closest_mode(
            &[system_mode(1, 1920, 1080, u32::MAX, u32::MAX, 60.0)],
            &VdMode {
                scale: 2.0,
                ..requested
            }
        ),
        None
    );
}

#[test]
fn missing_mode_confirmation_preserves_the_request() {
    let requested = VdMode {
        width_px: 2732,
        height_px: 2048,
        scale: 2.0,
        refresh_hz: 30,
    };
    assert_eq!(vdisplay::reported_mode(requested, None, None), requested);
    assert_eq!(
        vdisplay::reported_mode(requested, Some(vdisplay::DisplayMode::default()), None),
        requested
    );
}

#[test]
fn unconfirmed_report_keeps_requested_hidpi_when_process_data_is_stale() {
    let requested = VdMode {
        width_px: 2732,
        height_px: 2048,
        scale: 2.0,
        refresh_hz: 30,
    };
    // macOS 26.1 can report the old 1x geometry in the creating process. The
    // configured HiDPI request remains authoritative while mode confirmation
    // is pending; platform enumeration is evidence only.
    let enumerated = DisplayInfo {
        id: 1,
        name: "Virtual display".to_owned(),
        width_px: 1366,
        height_px: 1024,
        scale: 1.0,
        x: 0,
        y: 0,
        refresh_hz: 60,
        primary: false,
        rotation: 0,
    };
    assert_eq!(
        vdisplay::reported_mode(requested, None, Some(&enumerated)),
        requested
    );
    assert_eq!(
        vdisplay::reported_mode(
            requested,
            Some(system_mode(1, 1366, 1024, 1366, 1024, 60.0)),
            None
        ),
        requested
    );
    // Even matching raw data is not accepted on the timeout path; a measured
    // refresh is returned only by configure_mode after a usable mode list.
    assert_eq!(
        vdisplay::reported_mode(
            requested,
            Some(system_mode(2, 2732, 2048, 1366, 1024, 60.0)),
            Some(&enumerated)
        ),
        requested
    );
}

#[test]
fn unknown_refresh_keeps_requested_geometry_in_unconfirmed_report() {
    let requested = VdMode {
        width_px: 2732,
        height_px: 2048,
        scale: 2.0,
        refresh_hz: 30,
    };
    let observed = system_mode(1, 1366, 1024, 1366, 1024, 0.0);
    assert_eq!(observed.actual(), None, "raw timing is still unknown");
    assert_eq!(
        vdisplay::reported_mode(requested, Some(observed), None),
        requested,
        "stale geometry and unknown timing keep the configured request"
    );
}
