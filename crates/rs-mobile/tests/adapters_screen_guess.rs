//! Monitor layout guess for VNC bounding-box frames.

use std::time::{Duration, Instant};

use rs_mobile::rscore::adapters::{
    dto::{AdapterScreen, ScreenGuess},
    screen_guess::{guess, Bgra, BlackMask, Guesser, Pixels, Verdict, CONFIRM_GAP, MAX_SAMPLES},
};

const BLACK: [u8; 4] = [0, 0, 0, 255];
const NEAR_BLACK: [u8; 4] = [1, 1, 1, 255];
const WALL: [u8; 4] = [90, 60, 30, 255];
const OTHER: [u8; 4] = [30, 120, 200, 255];

/// A BGRA frame, all black until filled.
struct Frame {
    width: u32,
    height: u32,
    stride: usize,
    pixels: Vec<u8>,
}

impl Frame {
    fn new(width: u32, height: u32) -> Self {
        // Rows padded by one pixel so stride != width * 4.
        let stride = (width as usize + 1) * 4;
        let mut frame = Self {
            width,
            height,
            stride,
            pixels: vec![0; stride * height as usize],
        };
        frame.fill(0, 0, width, height, BLACK);
        frame
    }

    fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, bgra: [u8; 4]) -> &mut Self {
        for row in y..y + h {
            for col in x..x + w {
                let at = row as usize * self.stride + col as usize * 4;
                self.pixels[at..at + 4].copy_from_slice(&bgra);
            }
        }
        self
    }

    fn bgra(&self) -> Bgra<'_> {
        Bgra::new(&self.pixels, self.width, self.height, self.stride).expect("valid frame")
    }

    fn guess(&self) -> Vec<AdapterScreen> {
        guess(&self.bgra())
    }

    /// The whole frame drawn into a fresh guesser.
    fn guesser(&self) -> Guesser {
        let mut guesser = Guesser::new(self.width, self.height);
        guesser.put(0, 0, self.width, self.height, &self.pixels, self.stride);
        guesser
    }
}

fn gap(x: u32, y: u32, width: u32, height: u32) -> AdapterScreen {
    AdapterScreen {
        x,
        y,
        width,
        height,
        guess: Some(ScreenGuess::BlackGap),
    }
}

fn half(x: u32, y: u32, width: u32, height: u32) -> AdapterScreen {
    AdapterScreen {
        guess: Some(ScreenGuess::Aspect),
        ..gap(x, y, width, height)
    }
}

/// 1280×800 on the left, 800×600 centred on its right: black above and below.
fn side_by_side() -> Frame {
    let mut frame = Frame::new(2080, 800);
    frame
        .fill(0, 0, 1280, 800, WALL)
        .fill(1280, 100, 800, 600, OTHER);
    frame
}

/// 1280×800 on top, 800×600 below it, 240 px in: black left and right.
fn stacked() -> Frame {
    let mut frame = Frame::new(1280, 1400);
    frame
        .fill(0, 0, 1280, 800, WALL)
        .fill(240, 800, 800, 600, OTHER);
    frame
}

#[test]
fn side_by_side_different_heights() {
    assert_eq!(
        side_by_side().guess(),
        [gap(0, 0, 1280, 800), gap(1280, 100, 800, 600)]
    );
}

#[test]
fn side_by_side_bottom_aligned_three_screens() {
    let mut frame = Frame::new(2880, 900);
    frame
        .fill(0, 200, 800, 700, WALL)
        .fill(800, 0, 1280, 900, OTHER)
        .fill(2080, 300, 800, 600, WALL);
    assert_eq!(
        frame.guess(),
        [
            gap(0, 200, 800, 700),
            gap(800, 0, 1280, 900),
            gap(2080, 300, 800, 600),
        ]
    );
}

#[test]
fn stacked_different_widths() {
    assert_eq!(
        stacked().guess(),
        [gap(0, 0, 1280, 800), gap(240, 800, 800, 600)]
    );
}

#[test]
fn same_height_side_by_side_splits_by_aspect() {
    let mut frame = Frame::new(2560, 720);
    frame
        .fill(0, 0, 1280, 720, WALL)
        .fill(1280, 0, 1280, 720, OTHER);
    // 2560 / 720 > 2.4: left and right halves.
    assert_eq!(
        frame.guess(),
        [half(0, 0, 1280, 720), half(1280, 0, 1280, 720)]
    );
}

#[test]
fn same_width_stacked_splits_by_aspect() {
    let mut frame = Frame::new(1024, 1536);
    frame
        .fill(0, 0, 1024, 768, WALL)
        .fill(0, 768, 1024, 768, OTHER);
    // 0.65 <= 1024 / 1536 < 0.9: top and bottom halves.
    assert_eq!(
        frame.guess(),
        [half(0, 0, 1024, 768), half(0, 768, 1024, 768)]
    );
}

#[test]
fn ultrawide_and_portrait_single_screens_are_not_split() {
    // 21:9 ultrawide (≈2.39) and portrait 16:10 / 16:9 screens are one display.
    for (width, height) in [(3440, 1440), (2560, 1080), (1200, 1920), (1080, 1920)] {
        let mut frame = Frame::new(width, height);
        frame.fill(0, 0, width, height, WALL);
        assert_eq!(frame.guess(), [], "{width}x{height}");
    }
}

#[test]
fn one_screen_is_not_split() {
    let mut frame = Frame::new(1920, 1080);
    frame.fill(0, 0, 1920, 1080, WALL);
    assert_eq!(frame.guess(), []);
    // Halves below the minimum monitor size are not split either.
    let mut small = Frame::new(1200, 400);
    small.fill(0, 0, 1200, 400, WALL);
    assert_eq!(small.guess(), []);
    assert_eq!(Frame::new(30, 10).guess(), []);
}

#[test]
fn near_black_gaps_are_wallpaper() {
    // The side-by-side shape in a 16:9 frame, but the "gap" is (1, 1, 1).
    let mut frame = Frame::new(1920, 1080);
    frame
        .fill(0, 0, 1920, 1080, NEAR_BLACK)
        .fill(0, 0, 1200, 1080, WALL)
        .fill(1200, 200, 720, 600, OTHER);
    assert_eq!(frame.guess(), []);
}

#[test]
fn black_block_off_the_edges_is_content() {
    let mut frame = Frame::new(1920, 1080);
    frame
        .fill(0, 0, 1920, 1080, WALL)
        .fill(700, 1, 1219, 1078, BLACK);
    assert_eq!(frame.guess(), []);
}

#[test]
fn small_black_corner_is_content() {
    // Touches two edges, but under 3 % of the frame.
    let mut frame = Frame::new(1920, 1080);
    frame
        .fill(0, 0, 1920, 1080, WALL)
        .fill(1200, 1000, 720, 80, BLACK);
    assert_eq!(frame.guess(), []);
}

#[test]
fn ragged_black_edge_is_content() {
    // Dark wallpaper with pure black reaching the bottom edge in many uneven
    // columns: no clean monitor boundary.
    let mut frame = Frame::new(1920, 1080);
    frame.fill(0, 0, 1920, 1080, WALL);
    for x in (0..1920).step_by(8) {
        let h = 40 + (x * 37) % 300;
        frame.fill(x, 1080 - h, 4, h, BLACK);
    }
    assert_eq!(frame.guess(), []);
}

#[test]
fn black_specks_at_an_edge_do_not_move_the_split() {
    let mut frame = side_by_side();
    // Black content touching the top edge of the left monitor, and the
    // bottom of the right one.
    frame
        .fill(300, 0, 20, 50, BLACK)
        .fill(1270, 0, 10, 30, BLACK)
        .fill(1500, 680, 30, 20, BLACK);
    assert_eq!(
        frame.guess(),
        [gap(0, 0, 1280, 800), gap(1280, 100, 800, 600)]
    );
}

#[test]
fn bgra_rejects_short_buffers() {
    let pixels = vec![0; 16];
    assert!(Bgra::new(&pixels, 2, 2, 8).is_some());
    assert!(Bgra::new(&pixels, 2, 3, 8).is_none());
    assert!(Bgra::new(&pixels, 3, 1, 8).is_none());
}

#[test]
fn mask_follows_put_fill_and_copy() {
    let frame = side_by_side();
    let mut mask = BlackMask::new(frame.width, frame.height);
    assert!(mask.is_black(0, 0));
    mask.put(0, 0, frame.width, frame.height, &frame.pixels, frame.stride);
    assert_eq!(guess(&mask), frame.guess());

    let mut mirror = Frame::new(32, 8);
    let mut mask = BlackMask::new(32, 8);
    let fills = [
        (0, 0, 32, 8, WALL),
        (2, 1, 10, 3, BLACK),
        (20, 4, 40, 40, BLACK),
        (5, 6, 3, 1, NEAR_BLACK),
    ];
    for (x, y, w, h, bgra) in fills {
        mask.fill(x, y, w, h, bgra);
        let (w, h) = (w.min(32 - x), h.min(8 - y));
        mirror.fill(x, y, w, h, bgra);
    }
    // Overlapping copies both ways.
    for (sx, sy, dx, dy) in [(1, 0, 4, 2), (6, 3, 2, 1)] {
        mask.copy(sx, sy, dx, dy, 12, 5);
        let before = mirror.pixels.clone();
        for row in 0..5u32.min(8 - dy).min(8 - sy) {
            for col in 0..12 {
                let from = (sy + row) as usize * mirror.stride + (sx + col) as usize * 4;
                let to = (dy + row) as usize * mirror.stride + (dx + col) as usize * 4;
                mirror.pixels[to..to + 4].copy_from_slice(&before[from..from + 4]);
            }
        }
    }
    let view = mirror.bgra();
    for y in 0..8 {
        for x in 0..32 {
            assert_eq!(mask.is_black(x, y), view.is_black(x, y), "({x}, {y})");
        }
    }
}

#[test]
fn two_agreeing_samples_a_second_apart() {
    let t0 = Instant::now();
    let mut guesser = side_by_side().guesser();
    assert_eq!(guesser.commit(true, t0), Verdict::Wait);
    assert_eq!(guesser.poll(t0 + CONFIRM_GAP / 2), Verdict::Wait);
    assert_eq!(
        guesser.poll(t0 + CONFIRM_GAP),
        Verdict::Agreed(vec![gap(0, 0, 1280, 800), gap(1280, 100, 800, 600)])
    );
}

#[test]
fn one_screen_agrees_on_nothing() {
    let mut frame = Frame::new(1920, 1080);
    frame.fill(0, 0, 1920, 1080, WALL);
    let t0 = Instant::now();
    let mut guesser = frame.guesser();
    assert_eq!(guesser.commit(true, t0), Verdict::Wait);
    assert_eq!(
        guesser.commit(false, t0 + CONFIRM_GAP),
        Verdict::Agreed(Vec::new())
    );
}

#[test]
fn disagreeing_samples_are_not_reported() {
    let t0 = Instant::now();
    let frame = side_by_side();
    let mut guesser = frame.guesser();
    assert_eq!(guesser.commit(true, t0), Verdict::Wait);
    // A second later the black gap is covered: the aspect split disagrees.
    guesser.fill(1280, 0, 800, 100, OTHER);
    guesser.fill(1280, 700, 800, 100, OTHER);
    let t1 = t0 + CONFIRM_GAP;
    assert_eq!(guesser.commit(true, t1), Verdict::Wait);
    // The next sample waits a full second after the disagreeing one.
    assert_eq!(guesser.poll(t1 + CONFIRM_GAP / 2), Verdict::Wait);
    assert_eq!(
        guesser.poll(t1 + CONFIRM_GAP),
        Verdict::Agreed(vec![half(0, 0, 1040, 800), half(1040, 0, 1040, 800)])
    );
}

#[test]
fn no_sample_while_an_update_is_drawn() {
    let t0 = Instant::now();
    let mut guesser = side_by_side().guesser();
    assert_eq!(guesser.commit(true, t0), Verdict::Wait);
    guesser.fill(0, 0, 10, 10, OTHER);
    assert_eq!(guesser.poll(t0 + CONFIRM_GAP * 3), Verdict::Wait);
    assert!(matches!(
        guesser.commit(true, t0 + CONFIRM_GAP * 3),
        Verdict::Agreed(_)
    ));
}

#[test]
fn first_sample_waits_for_pixels() {
    let t0 = Instant::now();
    let mut guesser = side_by_side().guesser();
    // Updates without pixels (cursor only) and polls take no sample.
    assert_eq!(guesser.commit(false, t0), Verdict::Wait);
    assert_eq!(guesser.poll(t0 + CONFIRM_GAP * 2), Verdict::Wait);
    assert_eq!(guesser.commit(true, t0 + CONFIRM_GAP * 2), Verdict::Wait);
    assert!(matches!(
        guesser.poll(t0 + CONFIRM_GAP * 3),
        Verdict::Agreed(_)
    ));
}

#[test]
fn gives_up_when_samples_keep_changing() {
    let mut guesser = side_by_side().guesser();
    let mut now = Instant::now();
    assert_eq!(guesser.commit(true, now), Verdict::Wait);
    for sample in 2..=MAX_SAMPLES {
        // Alternate between the gap layout and a covered gap.
        let colour = if sample % 2 == 0 { OTHER } else { BLACK };
        guesser.fill(1280, 0, 800, 100, colour);
        now += CONFIRM_GAP + Duration::from_millis(1);
        let expected = if sample == MAX_SAMPLES {
            Verdict::GaveUp
        } else {
            Verdict::Wait
        };
        assert_eq!(guesser.commit(true, now), expected, "sample {sample}");
    }
}

#[test]
fn resume_waits_for_full_coverage_before_sampling() {
    let now = Instant::now();
    let mut guesser = Guesser::after_resume(1281, 480);
    guesser.fill(0, 0, 640, 480, WALL);
    assert_eq!(guesser.commit(true, now), Verdict::Wait);
    assert_eq!(guesser.poll(now + CONFIRM_GAP * 20), Verdict::Wait);
    // CopyRect cannot establish that its source was refreshed.
    guesser.copy(0, 0, 640, 0, 640, 480);
    assert_eq!(guesser.commit(true, now + CONFIRM_GAP * 21), Verdict::Wait);
    guesser.fill(640, 0, 641, 240, OTHER);
    assert_eq!(guesser.commit(true, now + CONFIRM_GAP * 22), Verdict::Wait);
    assert_eq!(guesser.poll(now + CONFIRM_GAP * 23), Verdict::Wait);
    guesser.fill(640, 240, 641, 240, OTHER);
    assert_eq!(guesser.commit(true, now + CONFIRM_GAP * 24), Verdict::Wait);
    assert!(matches!(
        guesser.poll(now + CONFIRM_GAP * 25),
        Verdict::Agreed(_)
    ));
}
