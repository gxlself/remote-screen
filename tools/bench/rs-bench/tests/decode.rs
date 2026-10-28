use rs_bench::decode::{
    change_rect, decode_at, sample_mean, DecodeError, Rect, TrackEvent, Tracker, TrackerConfig,
};
use rs_bench::synth::{
    add_band, add_noise, block_quantize, box_blur3, place, viewport, Background, Placement,
    Xorshift32,
};
use rs_bench::timecode::{cell_rect, quiet_probes_norm, render_calib, Code, GrayImage, PxRect};

const VIEW_W: u32 = 1920;
const VIEW_H: u32 = 1080;
const T: Code = Code::Time(5_800_804_987);
const REMOTE: Rect = Rect {
    x: 100.0,
    y: 120.0,
    w: 1280.0,
    h: 720.0,
};

fn full_rect(width: u32, height: u32) -> Rect {
    Rect {
        x: 0.0,
        y: 0.0,
        w: width as f64,
        h: height as f64,
    }
}

fn view() -> GrayImage {
    viewport(VIEW_W, VIEW_H, Background::Solid(40), Some(T))
}

fn window(source: &GrayImage, rect: Rect) -> GrayImage {
    let mut img = place(
        source,
        &Placement {
            win_w: 1600,
            win_h: 1000,
            rect,
            fill: 10,
        },
    );
    add_band(&mut img, 0, 60, 240);
    img
}

fn calibration_frame(white: bool, seed: u32, rect: Rect) -> GrayImage {
    let mut frame = window(&render_calib(VIEW_W, VIEW_H, white), rect);
    add_noise(&mut frame, 6, seed);
    frame
}

fn assert_rect(actual: Rect, expected: Rect, tolerance: f64) {
    for (name, got, want) in [
        ("x", actual.x, expected.x),
        ("y", actual.y, expected.y),
        ("w", actual.w, expected.w),
        ("h", actual.h, expected.h),
    ] {
        assert!(
            (got - want).abs() <= tolerance,
            "{name}: {got} != {want} (tolerance {tolerance})"
        );
    }
}

fn locked_tracker() -> Tracker {
    let mut tracker = Tracker::new(TrackerConfig::default());
    for i in 0..9 {
        tracker.push(&calibration_frame(i / 3 % 2 == 0, i, REMOTE));
    }
    assert!(tracker.locked().is_some(), "calibration must lock");
    tracker
}

fn paint_probe(img: &mut GrayImage, probe: (f64, f64), radius: u32, value: u8) {
    let x = (probe.0 * img.width as f64).round() as u32;
    let y = (probe.1 * img.height as f64).round() as u32;
    img.fill_rect(
        PxRect {
            x0: x - radius,
            y0: y - radius,
            x1: x + radius + 1,
            y1: y + radius + 1,
        },
        value,
    );
}

#[test]
fn decode_exact() {
    assert_eq!(decode_at(&view(), full_rect(VIEW_W, VIEW_H)), Ok(T));
}

#[test]
fn decode_placed_blur_noise() {
    let rect = Rect {
        x: 120.0,
        y: 145.0,
        w: 1200.0,
        h: 675.0,
    };
    let image = place(
        &view(),
        &Placement {
            win_w: 1440,
            win_h: 900,
            rect,
            fill: 20,
        },
    );
    let mut image = box_blur3(&image);
    add_noise(&mut image, 12, 7);
    assert_eq!(decode_at(&image, rect), Ok(T));
}

#[test]
fn decode_small() {
    let rect = full_rect(640, 360);
    let image = place(
        &view(),
        &Placement {
            win_w: 640,
            win_h: 360,
            rect,
            fill: 0,
        },
    );
    assert_eq!(decode_at(&image, rect), Ok(T));
}

#[test]
fn decode_backgrounds() {
    for bg in [
        Background::Noise { seed: 3 },
        Background::Stripes {
            period: 37,
            offset: 5,
        },
    ] {
        let image = viewport(VIEW_W, VIEW_H, bg, Some(T));
        assert_eq!(decode_at(&image, full_rect(VIEW_W, VIEW_H)), Ok(T));
    }
}

#[test]
fn low_contrast() {
    assert_eq!(
        decode_at(
            &GrayImage::new(VIEW_W, VIEW_H, 128),
            full_rect(VIEW_W, VIEW_H)
        ),
        Err(DecodeError::LowContrast)
    );
}

#[test]
fn wrong_rect_never_wrong_value() {
    let rect = Rect {
        x: 2.0 * VIEW_W as f64 / 64.0,
        ..full_rect(VIEW_W, VIEW_H)
    };
    assert!(decode_at(&view(), rect).is_err());
}

#[test]
fn flipped_cell_invalid() {
    let mut image = view();
    let cell = cell_rect(3, 2, VIEW_W, VIEW_H);
    for y in cell.y0..cell.y1 {
        for x in cell.x0..cell.x1 {
            image.set(x, y, 255 - image.get(x, y));
        }
    }
    assert_eq!(
        decode_at(&image, full_rect(VIEW_W, VIEW_H)),
        Err(DecodeError::Invalid)
    );
}

#[test]
fn quiet_zone_checked() {
    let mut image = view();
    paint_probe(&mut image, quiet_probes_norm()[0], 5, 255);
    assert_eq!(
        decode_at(&image, full_rect(VIEW_W, VIEW_H)),
        Err(DecodeError::QuietZone)
    );
}

#[test]
fn quiet_zone_threshold_and_all_probes() {
    for probe in quiet_probes_norm() {
        let mut image = view();
        paint_probe(&mut image, probe, 5, 84);
        assert_eq!(decode_at(&image, full_rect(VIEW_W, VIEW_H)), Ok(T));
        paint_probe(&mut image, probe, 5, 85);
        assert_eq!(
            decode_at(&image, full_rect(VIEW_W, VIEW_H)),
            Err(DecodeError::QuietZone)
        );
    }
    let mut small = viewport(640, 360, Background::Solid(40), Some(T));
    paint_probe(&mut small, quiet_probes_norm()[0], 2, 255);
    assert_eq!(
        decode_at(&small, full_rect(640, 360)),
        Err(DecodeError::QuietZone)
    );
}

#[test]
fn out_of_bounds() {
    let image = view();
    for x in [-200.0, -74.0] {
        assert_eq!(
            decode_at(
                &image,
                Rect {
                    x,
                    ..full_rect(VIEW_W, VIEW_H)
                }
            ),
            Err(DecodeError::OutOfBounds)
        );
    }
    for rect in [
        Rect {
            w: 0.0,
            ..full_rect(VIEW_W, VIEW_H)
        },
        Rect {
            h: -1.0,
            ..full_rect(VIEW_W, VIEW_H)
        },
        Rect {
            x: f64::NAN,
            ..full_rect(VIEW_W, VIEW_H)
        },
        Rect {
            y: f64::INFINITY,
            ..full_rect(VIEW_W, VIEW_H)
        },
    ] {
        assert_eq!(decode_at(&image, rect), Err(DecodeError::OutOfBounds));
    }
}

#[test]
fn change_rect_finds_remote() {
    let white = window(&render_calib(VIEW_W, VIEW_H, true), REMOTE);
    let black = window(&render_calib(VIEW_W, VIEW_H, false), REMOTE);
    assert_rect(
        change_rect(&white, &black).expect("remote rectangle"),
        REMOTE,
        3.0,
    );
}

#[test]
fn change_rect_none() {
    let before = GrayImage::new(1600, 1000, 0);
    assert_eq!(change_rect(&before, &before), None);
    let mut after = before.clone();
    after.fill_rect(
        PxRect {
            x0: 200,
            y0: 200,
            x1: 210,
            y1: 210,
        },
        255,
    );
    assert_eq!(change_rect(&before, &after), None);
    assert_eq!(change_rect(&before, &GrayImage::new(800, 600, 0)), None);
    assert_eq!(
        change_rect(&GrayImage::new(0, 0, 0), &GrayImage::new(0, 0, 0)),
        None
    );
}

#[test]
fn tracker_locks_and_reads() {
    let mut tracker = Tracker::new(TrackerConfig::default());
    assert_eq!(tracker.locked(), None);
    let mut locks = 0;
    for i in 0..9 {
        let event = tracker.push(&calibration_frame(i / 3 % 2 == 0, i, REMOTE));
        match event {
            TrackEvent::Locked {
                rect,
                host_w,
                host_h,
            } => {
                locks += 1;
                assert_eq!((host_w, host_h), (VIEW_W, VIEW_H));
                assert_rect(rect, REMOTE, REMOTE.w * 0.01);
                assert_eq!(tracker.locked(), Some((rect, host_w, host_h)));
            }
            TrackEvent::Searching => assert!([0, 1, 2, 4, 5].contains(&i)),
            TrackEvent::Calibrating => assert_eq!(i, 3),
            TrackEvent::Calib => assert!(i >= 7),
            other => panic!("unexpected calibration event on frame {i}: {other:?}"),
        }
    }
    assert_eq!(locks, 1);
    for i in 0..5u64 {
        let source = viewport(
            VIEW_W,
            VIEW_H,
            Background::Noise { seed: i as u32 + 9 },
            Some(Code::Time(1000 + i)),
        );
        let mut frame = window(&source, REMOTE);
        add_noise(&mut frame, 6, i as u32 + 20);
        assert_eq!(tracker.push(&frame), TrackEvent::Time(1000 + i));
    }
    let end = viewport(VIEW_W, VIEW_H, Background::Solid(40), Some(Code::End));
    assert_eq!(tracker.push(&window(&end, REMOTE)), TrackEvent::End);
}

#[test]
fn tracker_rejects_aspect() {
    let mut tracker = Tracker::new(TrackerConfig::default());
    let rect = Rect {
        x: 100.0,
        y: 100.0,
        w: 1280.0,
        h: 900.0,
    };
    for i in 0..12 {
        let event = tracker.push(&calibration_frame(i % 2 == 0, i, rect));
        assert_eq!(event, TrackEvent::Searching, "frame {i}");
    }
    assert_eq!(tracker.locked(), None);
}

#[test]
fn tracker_size_change_fails() {
    let mut tracker = locked_tracker();
    let lock = tracker.locked();
    assert_eq!(
        tracker.push(&GrayImage::new(800, 600, 0)),
        TrackEvent::Fail(DecodeError::OutOfBounds)
    );
    assert_eq!(tracker.locked(), lock);
    assert_eq!(
        tracker.push(&window(&view(), REMOTE)),
        TrackEvent::Time(5_800_804_987)
    );
}

#[test]
fn robustness_sweep() {
    let values = [Code::Time(0), T, Code::Time((1 << 36) - 1)];
    let sources = values.map(|code| viewport(VIEW_W, VIEW_H, Background::Solid(40), Some(code)));
    let mut checked = 0;
    let mut failures = Vec::new();
    for scale in [1.0, 0.75, 0.5, 0.4] {
        let width = (VIEW_W as f64 * scale) as u32;
        let height = (VIEW_H as f64 * scale) as u32;
        let rect = full_rect(width, height);
        for (source, code) in sources.iter().zip(values) {
            let scaled = place(
                source,
                &Placement {
                    win_w: width,
                    win_h: height,
                    rect,
                    fill: 20,
                },
            );
            for noise in [0, 16] {
                for blur in [false, true] {
                    for quantize in [false, true] {
                        if scale == 0.4 && quantize {
                            continue;
                        }
                        let mut image = if blur {
                            box_blur3(&scaled)
                        } else {
                            scaled.clone()
                        };
                        add_noise(&mut image, noise, 7);
                        if quantize {
                            block_quantize(&mut image, 8, 24);
                        }
                        let result = decode_at(&image, rect);
                        checked += 1;
                        if result != Ok(code) {
                            failures.push(format!("scale={scale} noise={noise} blur={blur} quantize={quantize} {code:?}: {result:?}"));
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 84);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    println!("robustness sweep: {checked}/84 decoded correctly");
}

#[test]
fn no_false_positive_on_noise() {
    let mut decoded = Vec::new();
    for seed in 1..=500 {
        let image = viewport(VIEW_W, VIEW_H, Background::Noise { seed }, None);
        if let Ok(code) = decode_at(&image, full_rect(VIEW_W, VIEW_H)) {
            decoded.push((seed, code));
        }
    }
    assert!(decoded.is_empty(), "false positives: {decoded:?}");
    println!("pure noise: 0/500 false positives");
}

#[test]
fn push_is_fast_after_lock() {
    let mut tracker = Tracker::new(TrackerConfig::default());
    for white in [true, false, true] {
        tracker.push(&render_calib(VIEW_W, VIEW_H, white));
    }
    assert!(tracker.locked().is_some());
    let frame = view();
    let started = std::time::Instant::now();
    for _ in 0..600 {
        assert_eq!(tracker.push(&frame), TrackEvent::Time(5_800_804_987));
    }
    let elapsed = started.elapsed();
    println!(
        "locked tracker: 600 frames of 1920x1080 in {elapsed:?} ({:?}/frame)",
        elapsed / 600
    );
}

#[test]
fn sample_mean_rounds_and_clips() {
    let img = GrayImage::from_raw(3, 2, vec![0, 1, 2, 3, 4, 5]).expect("fixture");
    assert_eq!(sample_mean(&img, 0.49, 0.0, 0), 0);
    assert_eq!(sample_mean(&img, 0.5, 0.0, 0), 1);
    assert_eq!(sample_mean(&img, 0.0, 0.0, 1), 2);
    assert_eq!(sample_mean(&img, -1.0, 0.0, 1), 2);
    assert_eq!(sample_mean(&img, -2.0, 0.0, 1), 0);
    assert_eq!(sample_mean(&img, 3.0, 1.0, 1), 4);
    assert_eq!(sample_mean(&img, 4.0, 1.0, 1), 0);
    assert_eq!(sample_mean(&img, 1.0, 0.0, u32::MAX), 3);
    for coordinate in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
        -f64::MAX,
    ] {
        assert_eq!(sample_mean(&img, coordinate, 0.0, 1), 0);
        assert_eq!(sample_mean(&img, 0.0, coordinate, 1), 0);
    }
    assert_eq!(sample_mean(&GrayImage::new(0, 0, 0), 0.0, 0.0, 1), 0);
}

#[test]
fn change_rect_bridges_short_gaps_and_trims_ends() {
    let before = GrayImage::new(200, 200, 0);
    let mut after = before.clone();
    after.fill_rect(
        PxRect {
            x0: 20,
            y0: 20,
            x1: 180,
            y1: 180,
        },
        255,
    );
    after.fill_rect(
        PxRect {
            x0: 20,
            y0: 40,
            x1: 180,
            y1: 44,
        },
        0,
    );
    after.fill_rect(
        PxRect {
            x0: 60,
            y0: 20,
            x1: 64,
            y1: 180,
        },
        0,
    );
    assert_eq!(
        change_rect(&before, &after),
        Some(Rect {
            x: 20.0,
            y: 20.0,
            w: 160.0,
            h: 160.0
        })
    );
    after.fill_rect(
        PxRect {
            x0: 20,
            y0: 96,
            x1: 180,
            y1: 102,
        },
        0,
    );
    assert_eq!(
        change_rect(&before, &after),
        Some(Rect {
            x: 20.0,
            y: 102.0,
            w: 160.0,
            h: 78.0
        })
    );
}

#[test]
fn change_rect_threshold_and_odd_sizes() {
    let before = GrayImage::new(17, 19, 0);
    assert_eq!(change_rect(&before, &GrayImage::new(17, 19, 95)), None);
    assert_eq!(
        change_rect(&before, &GrayImage::new(17, 19, 96)),
        Some(full_rect(18, 20))
    );
    assert_eq!(
        change_rect(&GrayImage::new(14, 20, 0), &GrayImage::new(14, 20, 255)),
        None
    );
    assert_eq!(
        change_rect(&GrayImage::new(20, 14, 0), &GrayImage::new(20, 14, 255)),
        None
    );
}

#[test]
fn tracker_preserves_candidate_and_averages_rectangles() {
    let mut tracker = Tracker::new(TrackerConfig::default());
    assert_eq!(
        tracker.push(&window(&render_calib(VIEW_W, VIEW_H, true), REMOTE)),
        TrackEvent::Searching
    );
    assert_eq!(
        tracker.push(&window(&render_calib(VIEW_W, VIEW_H, false), REMOTE)),
        TrackEvent::Calibrating
    );
    let moved = Rect {
        x: REMOTE.x + 10.0,
        ..REMOTE
    };
    assert_eq!(
        tracker.push(&window(&render_calib(VIEW_W, VIEW_H, false), moved)),
        TrackEvent::Searching
    );
    let event = tracker.push(&window(&render_calib(VIEW_W, VIEW_H, true), moved));
    assert_eq!(
        event,
        TrackEvent::Locked {
            rect: Rect {
                x: REMOTE.x + 5.0,
                ..REMOTE
            },
            host_w: VIEW_W,
            host_h: VIEW_H
        }
    );
}

#[test]
fn tracker_replaces_unstable_candidate() {
    let mut tracker = Tracker::new(TrackerConfig::default());
    tracker.push(&window(&render_calib(VIEW_W, VIEW_H, true), REMOTE));
    assert_eq!(
        tracker.push(&window(&render_calib(VIEW_W, VIEW_H, false), REMOTE)),
        TrackEvent::Calibrating
    );
    let moved = Rect {
        x: REMOTE.x + 100.0,
        ..REMOTE
    };
    assert_eq!(
        tracker.push(&window(&render_calib(VIEW_W, VIEW_H, false), moved)),
        TrackEvent::Searching
    );
    assert_eq!(
        tracker.push(&window(&render_calib(VIEW_W, VIEW_H, true), moved)),
        TrackEvent::Calibrating
    );
    assert_eq!(
        tracker.push(&window(&render_calib(VIEW_W, VIEW_H, false), moved)),
        TrackEvent::Locked {
            rect: moved,
            host_w: VIEW_W,
            host_h: VIEW_H
        }
    );
}

#[test]
fn tracker_keeps_lock_after_decode_failure() {
    let mut tracker = locked_tracker();
    let lock = tracker.locked();
    assert_eq!(
        tracker.push(&GrayImage::new(1600, 1000, 128)),
        TrackEvent::Fail(DecodeError::LowContrast)
    );
    assert_eq!(tracker.locked(), lock);
    assert_eq!(
        tracker.push(&window(&render_calib(VIEW_W, VIEW_H, true), REMOTE)),
        TrackEvent::Calib
    );
    assert_eq!(
        tracker.push(&window(&view(), REMOTE)),
        TrackEvent::Time(5_800_804_987)
    );
}

#[test]
fn synth_pixel_centers_bilinear_and_borders() {
    let source = GrayImage::from_raw(2, 2, vec![0, 64, 128, 255]).expect("fixture");
    let image = place(
        &source,
        &Placement {
            win_w: 4,
            win_h: 4,
            rect: full_rect(4, 4),
            fill: 20,
        },
    );
    assert_eq!(image.get(0, 0), 0);
    assert_eq!(image.get(1, 1), 52);
    assert_eq!(image.get(3, 3), 255);
    let image = place(
        &GrayImage::new(1, 1, 173),
        &Placement {
            win_w: 6,
            win_h: 6,
            rect: Rect {
                x: 1.5,
                y: 1.5,
                w: 3.0,
                h: 3.0,
            },
            fill: 20,
        },
    );
    assert_eq!(image.get(0, 1), 20);
    assert_eq!(image.get(1, 1), 173);
    assert_eq!(image.get(3, 3), 173);
    assert_eq!(image.get(4, 3), 20);
    assert_eq!(image.get(3, 4), 20);
    let image = place(
        &source,
        &Placement {
            win_w: 2,
            win_h: 2,
            rect: Rect {
                x: -1.0,
                y: -1.0,
                w: 2.0,
                h: 2.0,
            },
            fill: 20,
        },
    );
    assert_eq!(image.data, vec![255, 20, 20, 20]);
}

#[test]
fn synth_backgrounds_rng_and_effects() {
    let mut zero = Xorshift32(0);
    let mut one = Xorshift32(1);
    for expected in [270369, 67634689, 2647435461, 307599695, 2398689233] {
        assert_eq!(zero.next_u32(), expected);
        assert_eq!(one.next_u32(), expected);
    }
    let stripes = viewport(
        6,
        2,
        Background::Stripes {
            period: 3,
            offset: 2,
        },
        None,
    );
    assert_eq!(&stripes.data[..6], &[230, 25, 25, 25, 230, 230]);
    assert_eq!(&stripes.data[..6], &stripes.data[6..]);
    let mut band = GrayImage::new(3, 4, 0);
    add_band(&mut band, 1, 3, 200);
    add_band(&mut band, 7, 9, 100);
    add_band(&mut band, 3, 1, 100);
    assert_eq!(
        band.data,
        vec![0, 0, 0, 200, 200, 200, 200, 200, 200, 0, 0, 0]
    );
    let blurred = box_blur3(&GrayImage::from_raw(2, 1, vec![0, 90]).expect("fixture"));
    assert_eq!(blurred.data, vec![30, 60]);
    let mut blocks = GrayImage::from_raw(3, 1, vec![0, 100, 255]).expect("fixture");
    block_quantize(&mut blocks, 2, 24);
    assert_eq!(blocks.data, vec![36, 84, 252]);
    let mut saturated = GrayImage::new(2, 2, 255);
    block_quantize(&mut saturated, u32::MAX, 255);
    assert_eq!(saturated.data, vec![255; 4]);
    let mut noise = GrayImage::new(100, 2, 100);
    add_noise(&mut noise, 16, 7);
    assert!(noise.data.iter().all(|&v| (84..=116).contains(&v)));
    let mut duplicate = GrayImage::new(100, 2, 100);
    add_noise(&mut duplicate, 16, 7);
    assert_eq!(noise, duplicate);
    add_noise(&mut duplicate, 0, 10);
    assert_eq!(noise, duplicate);
}

#[test]
fn decode_error_strings() {
    for (error, text) in [
        (DecodeError::OutOfBounds, "out-of-bounds"),
        (DecodeError::LowContrast, "low-contrast"),
        (DecodeError::QuietZone, "quiet-zone"),
        (DecodeError::Invalid, "invalid"),
    ] {
        assert_eq!(error.as_str(), text);
    }
}
