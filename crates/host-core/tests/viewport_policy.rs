use host_core::pipeline::viewport::{
    contains, expand_crop, expand_roi, zoom_of, PolicyOutput, ViewMode, ViewReport, ViewportCaps,
    ViewportPolicy,
};
use host_core::platform::PixelRect;

const DISPLAY_W: u32 = 1920;
const DISPLAY_H: u32 = 1080;
const BOTH: ViewportCaps = ViewportCaps {
    crop: true,
    roi: true,
};
const CROP: ViewportCaps = ViewportCaps {
    crop: true,
    roi: false,
};
const ROI: ViewportCaps = ViewportCaps {
    crop: false,
    roi: true,
};

const fn rect(x: u32, y: u32, w: u32, h: u32) -> PixelRect {
    PixelRect { x, y, w, h }
}

const A: PixelRect = rect(800, 400, 480, 270);
const B: PixelRect = rect(816, 410, 480, 270);
const C: PixelRect = rect(1200, 600, 480, 270);
const Z2: PixelRect = rect(480, 270, 960, 540);
const Z18: PixelRect = rect(400, 240, 1066, 600);
const Z15: PixelRect = rect(300, 200, 1280, 720);
const Z12: PixelRect = rect(100, 50, 1600, 900);
const Z13: PixelRect = rect(200, 100, 1476, 831);

fn report(seq: u32, rect: Option<PixelRect>, settled: bool) -> ViewReport {
    ViewReport {
        seq,
        rect,
        settled,
        view_w: 2532,
        view_h: 1170,
        crop_ok: true,
    }
}

fn some_crop(r: PixelRect) -> PolicyOutput {
    PolicyOutput {
        crop: Some(Some(r)),
        roi: None,
    }
}

#[test]
fn zoom_values() {
    assert_eq!(zoom_of(None, DISPLAY_W, DISPLAY_H), 1.0);
    assert_eq!(zoom_of(Some(A), DISPLAY_W, DISPLAY_H), 4.0);
    assert_eq!(
        zoom_of(Some(rect(0, 0, 1920, 1080)), DISPLAY_W, DISPLAY_H),
        1.0
    );
    assert!((zoom_of(Some(Z13), DISPLAY_W, DISPLAY_H) - 1.3002).abs() <= 1e-3);
}

#[test]
fn expand_crop_values() {
    assert_eq!(
        expand_crop(A, DISPLAY_W, DISPLAY_H),
        rect(736, 352, 608, 352)
    );
    assert_eq!(
        expand_crop(C, DISPLAY_W, DISPLAY_H),
        rect(1136, 560, 608, 352)
    );
    assert_eq!(
        expand_crop(Z2, DISPLAY_W, DISPLAY_H),
        rect(352, 192, 1216, 688)
    );
    assert_eq!(
        expand_crop(rect(0, 0, 160, 90), DISPLAY_W, DISPLAY_H),
        rect(0, 0, 192, 128)
    );
    assert_eq!(
        expand_crop(rect(1800, 1000, 120, 80), DISPLAY_W, DISPLAY_H),
        rect(1776, 952, 144, 128)
    );
}

#[test]
fn expand_roi_values() {
    assert_eq!(
        expand_roi(A, DISPLAY_W, DISPLAY_H),
        rect(704, 320, 640, 384)
    );
    assert_eq!(
        expand_roi(B, DISPLAY_W, DISPLAY_H),
        rect(768, 320, 576, 448)
    );
    assert_eq!(
        expand_roi(C, DISPLAY_W, DISPLAY_H),
        rect(1152, 512, 576, 448)
    );
    assert_eq!(
        expand_roi(Z13, DISPLAY_W, DISPLAY_H),
        rect(0, 0, 1856, 1024)
    );
    assert_eq!(
        expand_roi(Z12, DISPLAY_W, DISPLAY_H),
        rect(0, 0, 1920, 1080)
    );
}

#[test]
fn contains_cases() {
    assert!(contains(expand_crop(A, DISPLAY_W, DISPLAY_H), B));
    assert!(!contains(expand_crop(A, DISPLAY_W, DISPLAY_H), C));
    assert!(contains(
        expand_crop(A, DISPLAY_W, DISPLAY_H),
        expand_crop(A, DISPLAY_W, DISPLAY_H)
    ));
}

#[test]
fn unsettled_does_not_crop() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    assert_eq!(
        p.on_report(0, report(1, Some(A), false)),
        PolicyOutput::default()
    );
    assert_eq!(p.mode(), ViewMode::Off);
}

#[test]
fn settled_enters_crop() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    p.on_report(0, report(1, Some(A), false));
    assert_eq!(
        p.on_report(300_000, report(2, Some(A), true)),
        some_crop(rect(736, 352, 608, 352))
    );
    assert_eq!(p.mode(), ViewMode::Crop);
    assert_eq!(p.crop_seq(), 2);
}

#[test]
fn pan_inside_keeps_crop() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    p.on_report(0, report(1, Some(A), true));
    assert_eq!(
        p.on_report(500_000, report(2, Some(B), false)),
        PolicyOutput::default()
    );
    assert_eq!(p.mode(), ViewMode::Crop);
}

#[test]
fn out_of_bounds_exit_waits_hold() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    p.on_report(0, report(1, Some(A), false));
    p.on_report(300_000, report(2, Some(A), true));
    assert_eq!(
        p.on_report(400_000, report(3, Some(C), false)),
        PolicyOutput::default()
    );
    assert_eq!(p.deadline(), Some(600_000));
    assert_eq!(p.poll(599_999), PolicyOutput::default());
    assert_eq!(
        p.poll(600_000),
        PolicyOutput {
            crop: Some(None),
            roi: None
        }
    );
    assert_eq!(p.mode(), ViewMode::Off);
}

#[test]
fn reenter_waits_interval() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    p.on_report(0, report(1, Some(A), false));
    p.on_report(300_000, report(2, Some(A), true));
    p.on_report(400_000, report(3, Some(C), false));
    p.poll(600_000);
    assert_eq!(
        p.on_report(900_000, report(4, Some(C), true)),
        PolicyOutput::default()
    );
    assert_eq!(p.deadline(), Some(1_600_000));
    assert_eq!(p.poll(1_600_000), some_crop(rect(1136, 560, 608, 352)));
}

#[test]
fn settled_move_changes_after_interval() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    p.on_report(0, report(1, Some(A), false));
    p.on_report(300_000, report(2, Some(A), true));
    assert_eq!(
        p.on_report(800_000, report(3, Some(C), true)),
        PolicyOutput::default()
    );
    assert_eq!(p.deadline(), Some(1_300_000));
    assert_eq!(p.poll(1_300_000), some_crop(rect(1136, 560, 608, 352)));
}

#[test]
fn zoom_out_hysteresis() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    assert_eq!(
        p.on_report(0, report(1, Some(Z2), true)),
        some_crop(rect(352, 192, 1216, 688))
    );
    assert_eq!(
        p.on_report(2_000_000, report(2, Some(Z18), true)),
        PolicyOutput::default()
    );
    assert_eq!(
        p.on_report(3_000_000, report(3, Some(Z15), true)),
        PolicyOutput {
            crop: Some(None),
            roi: None
        }
    );
}

#[test]
fn tighten_on_zoom_in() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    p.on_report(0, report(1, Some(Z2), true));
    assert_eq!(
        p.on_report(2_000_000, report(2, Some(A), true)),
        some_crop(rect(736, 352, 608, 352))
    );
}

#[test]
fn crop_ok_false_exits_now() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    p.on_report(0, report(1, Some(Z2), true));
    let mut r = report(2, Some(Z2), true);
    r.crop_ok = false;
    assert_eq!(
        p.on_report(100_000, r),
        PolicyOutput {
            crop: Some(None),
            roi: None
        }
    );
}

#[test]
fn full_rect_exits_with_hold() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    p.on_report(0, report(1, Some(Z2), true));
    assert_eq!(
        p.on_report(100_000, report(2, None, true)),
        PolicyOutput::default()
    );
    assert_eq!(p.deadline(), Some(300_000));
    assert_eq!(
        p.poll(300_000),
        PolicyOutput {
            crop: Some(None),
            roi: None
        }
    );
}

#[test]
fn roi_only_when_crop_unsupported() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, ROI);
    assert_eq!(
        p.on_report(0, report(1, Some(A), true)),
        PolicyOutput {
            crop: None,
            roi: Some(Some(rect(704, 320, 640, 384)))
        }
    );
    assert_eq!(p.mode(), ViewMode::Roi);
}

#[test]
fn roi_hysteresis() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, ROI);
    assert_eq!(
        p.on_report(0, report(1, Some(Z12), true)),
        PolicyOutput::default()
    );
    assert_eq!(
        p.on_report(1_000_000, report(2, Some(Z13), true)),
        PolicyOutput {
            crop: None,
            roi: Some(Some(rect(0, 0, 1856, 1024)))
        }
    );
    assert_eq!(
        p.on_report(2_000_000, report(3, Some(Z12), true)),
        PolicyOutput {
            crop: None,
            roi: Some(Some(rect(0, 0, 1920, 1080)))
        }
    );
    assert_eq!(
        p.on_report(3_000_000, report(4, None, true)),
        PolicyOutput {
            crop: None,
            roi: Some(None)
        }
    );
}

#[test]
fn roi_throttle() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, ROI);
    assert_eq!(
        p.on_report(0, report(1, Some(A), false)),
        PolicyOutput {
            crop: None,
            roi: Some(Some(rect(704, 320, 640, 384)))
        }
    );
    assert_eq!(
        p.on_report(100_000, report(2, Some(B), false)),
        PolicyOutput::default()
    );
    assert_eq!(p.deadline(), Some(200_000));
    assert_eq!(
        p.poll(200_000),
        PolicyOutput {
            crop: None,
            roi: Some(Some(rect(768, 320, 576, 448)))
        }
    );
}

#[test]
fn crop_clears_roi() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, BOTH);
    assert_eq!(
        p.on_report(0, report(1, Some(A), false)),
        PolicyOutput {
            crop: None,
            roi: Some(Some(rect(704, 320, 640, 384)))
        }
    );
    assert_eq!(
        p.on_report(300_000, report(2, Some(A), true)),
        PolicyOutput {
            crop: Some(Some(rect(736, 352, 608, 352))),
            roi: Some(None),
        }
    );
    assert_eq!(p.mode(), ViewMode::Crop);
}

#[test]
fn exit_restores_roi_immediately() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, BOTH);
    p.on_report(0, report(1, Some(A), false));
    p.on_report(300_000, report(2, Some(A), true));
    p.on_report(400_000, report(3, Some(C), false));
    assert_eq!(
        p.poll(600_000),
        PolicyOutput {
            crop: Some(None),
            roi: Some(Some(rect(1152, 512, 576, 448))),
        }
    );
    assert_eq!(p.mode(), ViewMode::Roi);
}

#[test]
fn poll_is_idempotent() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, ROI);
    let output = p.on_report(0, report(1, Some(A), false));
    assert!(output.roi.is_some());
    assert_eq!(p.poll(0), PolicyOutput::default());
}

#[test]
fn crop_failed_disables() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    p.on_report(0, report(1, Some(A), true));
    p.crop_failed(400_000);
    assert_eq!(p.mode(), ViewMode::Off);
    assert!(!p.caps().crop);
    assert_eq!(
        p.on_report(500_000, report(2, Some(A), true)),
        PolicyOutput::default()
    );
}

#[test]
fn set_caps_enables_roi() {
    let mut p = ViewportPolicy::new(DISPLAY_W, DISPLAY_H, CROP);
    assert_eq!(
        p.on_report(0, report(1, Some(A), false)),
        PolicyOutput::default()
    );
    assert_eq!(
        p.set_caps(10, BOTH),
        PolicyOutput {
            crop: None,
            roi: Some(Some(rect(704, 320, 640, 384)))
        }
    );
}
