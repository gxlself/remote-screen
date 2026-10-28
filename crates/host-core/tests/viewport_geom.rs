use host_core::pipeline::viewport_geom::{
    aspect_matches, crop_output_size, display_to_stream, focus_regions, rect_in_display,
    ROI_INSIDE_QP, ROI_OUTSIDE_QP,
};
use host_core::platform::{PixelRect, RoiRegion};

fn rect(x: u32, y: u32, w: u32, h: u32) -> PixelRect {
    PixelRect { x, y, w, h }
}

#[test]
fn crop_size_native() {
    assert_eq!(
        crop_output_size(
            rect(736, 352, 608, 352),
            rect(800, 400, 480, 270),
            (1920, 1080),
            (2532, 1170),
            (0, 0),
        ),
        (608, 352)
    );
}

#[test]
fn crop_size_budget_limited() {
    let output = crop_output_size(
        rect(0, 0, 2160, 1216),
        rect(0, 0, 1920, 1080),
        (1280, 720),
        (0, 0),
        (0, 0),
    );
    assert_eq!(output, (1278, 720));
    assert!(u64::from(output.0) * u64::from(output.1) <= u64::from(1280_u32) * 720);
}

#[test]
fn crop_size_view_limited() {
    assert_eq!(
        crop_output_size(
            rect(1472, 720, 1216, 688),
            rect(1600, 800, 960, 540),
            (1920, 1080),
            (480, 270),
            (0, 0),
        ),
        (608, 344)
    );
}

#[test]
fn crop_size_start_limit() {
    assert_eq!(
        crop_output_size(
            rect(1472, 720, 1216, 688),
            rect(1600, 800, 960, 540),
            (1920, 1080),
            (0, 0),
            (608, 0),
        ),
        (608, 344)
    );
}

#[test]
fn crop_size_never_upscales() {
    assert_eq!(
        crop_output_size(
            rect(0, 0, 1002, 602),
            rect(0, 0, 800, 480),
            (4000, 4000),
            (0, 0),
            (0, 0),
        ),
        (1002, 602)
    );
}

#[test]
fn crop_size_degenerate() {
    assert_eq!(
        crop_output_size(
            rect(0, 0, 0, 602),
            rect(0, 0, 800, 480),
            (4000, 4000),
            (0, 0),
            (0, 0),
        ),
        (2, 2)
    );
    let output = crop_output_size(
        rect(0, 0, 0, 602),
        rect(0, 0, 800, 480),
        (4000, 4000),
        (10, 10),
        (0, 0),
    );
    assert!(output.0 >= 2 && output.1 >= 2);
    assert_eq!(output.0 % 2, 0);
    assert_eq!(output.1 % 2, 0);
}

#[test]
fn crop_size_always_even_and_in_budget() {
    for width in [130, 256, 1000, 2160, 3840] {
        for height in [98, 300, 1216, 2160] {
            let output = crop_output_size(
                rect(0, 0, width, height),
                rect(0, 0, 1920, 1080),
                (1280, 720),
                (0, 0),
                (0, 0),
            );
            assert_eq!(output.0 % 2, 0, "width={width}, height={height}");
            assert_eq!(output.1 % 2, 0, "width={width}, height={height}");
            assert!(
                u64::from(output.0) * u64::from(output.1) <= u64::from(1280_u32) * 720,
                "width={width}, height={height}, output={output:?}"
            );
            assert!(
                aspect_matches(rect(0, 0, width, height), output),
                "width={width}, height={height}, output={output:?}"
            );
        }
    }
}

#[test]
fn to_stream_scaled() {
    assert_eq!(
        display_to_stream(rect(704, 320, 640, 384), (1920, 1080), (1280, 720), 16),
        rect(464, 208, 432, 272)
    );
}

#[test]
fn to_stream_clamped() {
    assert_eq!(
        display_to_stream(rect(1856, 1024, 64, 56), (1920, 1080), (1280, 720), 16),
        rect(1232, 672, 48, 48)
    );
}

#[test]
fn to_stream_identity() {
    let input = rect(704, 320, 640, 384);
    assert_eq!(
        display_to_stream(input, (1920, 1080), (1920, 1080), 1),
        input
    );
}

#[test]
fn focus_small_view_adds_outside() {
    assert_eq!(
        focus_regions(
            rect(704, 320, 640, 384),
            rect(800, 400, 480, 270),
            (1920, 1080),
            (1280, 720),
        ),
        vec![
            RoiRegion {
                rect: rect(464, 208, 432, 272),
                qp_delta: ROI_INSIDE_QP,
            },
            RoiRegion {
                rect: rect(0, 0, 1280, 720),
                qp_delta: ROI_OUTSIDE_QP,
            },
        ]
    );
}

#[test]
fn focus_large_view_inside_only() {
    assert_eq!(
        focus_regions(
            rect(704, 320, 640, 384),
            rect(0, 0, 1600, 900),
            (1920, 1080),
            (1280, 720),
        ),
        vec![RoiRegion {
            rect: rect(464, 208, 432, 272),
            qp_delta: ROI_INSIDE_QP,
        }]
    );
}

#[test]
fn focus_empty() {
    assert!(focus_regions(
        rect(704, 320, 0, 384),
        rect(800, 400, 480, 270),
        (1920, 1080),
        (1280, 720),
    )
    .is_empty());
}

#[test]
fn aspect_and_bounds() {
    let crop = rect(0, 0, 2160, 1216);
    assert!(aspect_matches(crop, (1278, 720)));
    assert!(!aspect_matches(crop, (1278, 600)));
    assert!(!rect_in_display(rect(1800, 0, 200, 10), (1920, 1080)));
}
