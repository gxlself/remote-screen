use std::sync::Arc;

use host_core::platform::{CapturedFrame, PixelRect};
use host_fake::SyntheticFrame;
use host_win::viewport_crop::{self, RegionFrame};
use host_win::vpmf_model::RectU;

fn rect(x: u32, y: u32, w: u32, h: u32) -> PixelRect {
    PixelRect { x, y, w, h }
}

fn frame() -> CapturedFrame {
    CapturedFrame {
        display_id: 1,
        width: 1920,
        height: 1080,
        capture_ts_us: 7,
        native: Arc::new(SyntheticFrame {
            frame_number: 42,
            capture_ts_us: 7,
        }),
    }
}

#[test]
fn tex_rect_identity() {
    assert_eq!(
        viewport_crop::texture_source_rect(rect(736, 352, 608, 352), (1920, 1080), (1920, 1080), 0,),
        RectU {
            left: 736,
            top: 352,
            right: 1344,
            bottom: 704,
        }
    );
}

#[test]
fn tex_rect_rotations() {
    let r = rect(100, 200, 300, 400);
    assert_eq!(
        viewport_crop::texture_source_rect(r, (1080, 1920), (1920, 1080), 90),
        RectU {
            left: 200,
            top: 680,
            right: 600,
            bottom: 980,
        }
    );
    assert_eq!(
        viewport_crop::texture_source_rect(r, (1920, 1080), (1920, 1080), 180),
        RectU {
            left: 1520,
            top: 480,
            right: 1820,
            bottom: 880,
        }
    );
    assert_eq!(
        viewport_crop::texture_source_rect(r, (1080, 1920), (1920, 1080), 270),
        RectU {
            left: 1320,
            top: 100,
            right: 1720,
            bottom: 400,
        }
    );
}

#[test]
fn tex_rect_scaled() {
    assert_eq!(
        viewport_crop::texture_source_rect(rect(736, 352, 608, 352), (3840, 2160), (1920, 1080), 0,),
        RectU {
            left: 368,
            top: 176,
            right: 672,
            bottom: 352,
        }
    );
}

#[test]
fn tex_rect_degenerate() {
    assert_eq!(
        viewport_crop::texture_source_rect(rect(1, 2, 3, 4), (0, 0), (1920, 1080), 0),
        RectU {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        }
    );
    assert_eq!(
        viewport_crop::texture_source_rect(
            rect(1800, 1000, 400, 200),
            (1920, 1080),
            (1920, 1080),
            0
        ),
        RectU {
            left: 1800,
            top: 1000,
            right: 1920,
            bottom: 1080,
        }
    );
}

#[test]
fn wrap_split_roundtrip() {
    let wrapped = viewport_crop::wrap_frame(frame(), Some((rect(736, 352, 608, 352), 1920, 1080)));
    let (native, region) = viewport_crop::split_native(wrapped.native.as_ref());
    assert_eq!(region, Some((rect(736, 352, 608, 352), 1920, 1080)));
    assert_eq!(
        native
            .downcast_ref::<SyntheticFrame>()
            .map(|f| f.frame_number),
        Some(42)
    );
    assert!(wrapped.native.downcast_ref::<RegionFrame>().is_some());

    let plain = viewport_crop::wrap_frame(frame(), None);
    let (native, region) = viewport_crop::split_native(plain.native.as_ref());
    assert_eq!(region, None);
    assert_eq!(
        native
            .downcast_ref::<SyntheticFrame>()
            .map(|f| f.frame_number),
        Some(42)
    );
}
