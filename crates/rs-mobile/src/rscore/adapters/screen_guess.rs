//! Monitor layout guess for VNC servers that send one bounding-box frame and
//! no ExtendedDesktopSize screens, like macOS Screen Sharing.
//!
//! Monitors of different sizes leave pure black (0, 0, 0) gaps in the
//! bounding box. Along one axis every line (column or row) has the same black
//! runs from both edges inside one monitor; a change of runs marks the next
//! monitor. Without such gaps a very wide or tall desktop is split in halves.

use std::time::{Duration, Instant};

use super::dto::{AdapterScreen, ScreenGuess};

/// Smallest monitor accepted, in pixels.
pub const MIN_WIDTH: u32 = 640;
pub const MIN_HEIGHT: u32 = 480;
/// Black gaps must cover at least this share of the bounding box, in percent.
pub const MIN_GAP_PERCENT: u64 = 3;
pub const MAX_SCREENS: usize = 4;
/// Without gaps, width / height above this splits left / right halves. Above 21:9
/// (≈2.39) so a single ultrawide stays whole; two 4:3 side by side are 2.67.
pub const WIDE_ASPECT: f64 = 2.4;
/// …and between these top / bottom halves: two stacked 4:3 / 16:10 / 16:9 screens
/// are 0.67 / 0.8 / 0.89, a single portrait 16:10 / 16:9 screen 0.63 / 0.56.
pub const TALL_ASPECT: f64 = 0.9;
pub const TALL_ASPECT_MIN: f64 = 0.65;
/// Two samples at least this far apart must agree before a layout is used.
pub const CONFIRM_GAP: Duration = Duration::from_secs(1);
/// Samples per frame size before giving up.
pub const MAX_SAMPLES: u32 = 10;
/// More changes of edge runs than this along an axis is content, not monitors.
const MAX_BANDS: usize = 64;

/// Read access to a frame for [`guess`].
pub trait Pixels {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    /// Pure black: B = G = R = 0, alpha ignored.
    fn is_black(&self, x: u32, y: u32) -> bool;
}

/// A BGRA8 frame, `stride` bytes per row.
pub struct Bgra<'a> {
    pixels: &'a [u8],
    width: u32,
    height: u32,
    stride: usize,
}

impl<'a> Bgra<'a> {
    /// None when `pixels` is too short for `height` rows of `width` pixels.
    pub fn new(pixels: &'a [u8], width: u32, height: u32, stride: usize) -> Option<Self> {
        let row = width as usize * 4;
        let needed = match height {
            0 => 0,
            _ => stride.checked_mul(height as usize - 1)?.checked_add(row)?,
        };
        (stride >= row && pixels.len() >= needed).then_some(Self {
            pixels,
            width,
            height,
            stride,
        })
    }
}

impl Pixels for Bgra<'_> {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn is_black(&self, x: u32, y: u32) -> bool {
        let at = y as usize * self.stride + x as usize * 4;
        self.pixels[at..at + 3] == [0, 0, 0]
    }
}

/// Monitors in the frame, top-left first; empty for one monitor.
pub fn guess(px: &impl Pixels) -> Vec<AdapterScreen> {
    black_gaps(px)
        .or_else(|| by_aspect(px.width(), px.height()))
        .unwrap_or_default()
}

/// Monitors side by side (columns), else stacked (rows).
fn black_gaps(px: &impl Pixels) -> Option<Vec<AdapterScreen>> {
    let (width, height) = (px.width(), px.height());
    if width < MIN_WIDTH || height < MIN_HEIGHT {
        return None;
    }
    let (top, bottom) = (column_runs(px, false), column_runs(px, true));
    if let Some(bands) = split(&top, &bottom, height, MIN_WIDTH, MIN_HEIGHT) {
        let screens = bands.iter().map(|band| {
            screen(
                band.start,
                band.near,
                band.len(),
                height - band.near - band.far,
            )
        });
        return Some(screens.collect());
    }
    let (left, right) = (row_runs(px, false), row_runs(px, true));
    let bands = split(&left, &right, width, MIN_HEIGHT, MIN_WIDTH)?;
    let screens = bands.iter().map(|band| {
        screen(
            band.near,
            band.start,
            width - band.near - band.far,
            band.len(),
        )
    });
    Some(screens.collect())
}

fn screen(x: u32, y: u32, width: u32, height: u32) -> AdapterScreen {
    AdapterScreen {
        x,
        y,
        width,
        height,
        guess: Some(ScreenGuess::BlackGap),
    }
}

/// Black pixels at the top (or bottom) of every column; `height` when the
/// whole column is black. Reads row by row.
fn column_runs(px: &impl Pixels, from_bottom: bool) -> Vec<u32> {
    let (width, height) = (px.width(), px.height());
    let mut runs = vec![height; width as usize];
    let mut open = width;
    for step in 0..height {
        let y = if from_bottom { height - 1 - step } else { step };
        for x in 0..width {
            if runs[x as usize] == height && !px.is_black(x, y) {
                runs[x as usize] = step;
                open -= 1;
            }
        }
        if open == 0 {
            break;
        }
    }
    runs
}

/// Black pixels at the left (or right) of every row; `width` when the whole
/// row is black.
fn row_runs(px: &impl Pixels, from_right: bool) -> Vec<u32> {
    let (width, height) = (px.width(), px.height());
    (0..height)
        .map(|y| {
            (0..width)
                .position(|step| {
                    let x = if from_right { width - 1 - step } else { step };
                    !px.is_black(x, y)
                })
                .map_or(width, |run| run as u32)
        })
        .collect()
}

/// Lines `start..end` along the axis with `near` / `far` black edge runs.
#[derive(Debug, Clone, Copy)]
struct Band {
    start: u32,
    end: u32,
    near: u32,
    far: u32,
}

impl Band {
    fn len(&self) -> u32 {
        self.end - self.start
    }

    /// `other`'s runs are no longer than ours: our extra black is content.
    fn covers(&self, other: &Band) -> bool {
        other.near <= self.near && other.far <= self.far
    }
}

/// Monitors along one axis from the edge runs of each line, or None.
/// `across` is the frame size across the axis.
fn split(
    near: &[u32],
    far: &[u32],
    across: u32,
    min_along: u32,
    min_across: u32,
) -> Option<Vec<Band>> {
    let mut bands: Vec<Band> = Vec::new();
    for (index, (&near, &far)) in near.iter().zip(far).enumerate() {
        let index = index as u32;
        match bands.last_mut() {
            Some(band) if band.near == near && band.far == far => band.end = index + 1,
            _ => {
                if bands.len() == MAX_BANDS {
                    return None;
                }
                bands.push(Band {
                    start: index,
                    end: index + 1,
                    near,
                    far,
                });
            }
        }
    }
    // A band too narrow for a monitor is black content at the edge of the
    // monitor beside it: fold it into a neighbour with shorter runs.
    while let Some(index) = (0..bands.len())
        .filter(|&index| bands[index].len() < min_along)
        .min_by_key(|&index| bands[index].len())
    {
        let narrow = bands[index];
        let neighbour = [index.checked_sub(1), Some(index + 1)]
            .into_iter()
            .flatten()
            .filter(|&other| other < bands.len() && narrow.covers(&bands[other]))
            .max_by_key(|&other| bands[other].len())?;
        let into = &mut bands[neighbour];
        into.start = into.start.min(narrow.start);
        into.end = into.end.max(narrow.end);
        bands.remove(index);
        bands.dedup_by(|next, band| {
            let same = band.near == next.near && band.far == next.far;
            if same {
                band.end = next.end;
            }
            same
        });
    }
    if !(2..=MAX_SCREENS).contains(&bands.len()) {
        return None;
    }
    let along = u64::from(bands.last()?.end);
    let mut black = 0;
    for band in &bands {
        let edges = band.near + band.far;
        if edges >= across || across - edges < min_across {
            return None;
        }
        black += u64::from(edges) * u64::from(band.len());
    }
    (black * 100 >= MIN_GAP_PERCENT * along * u64::from(across)).then_some(bands)
}

/// Halves of a very wide or tall frame, each at least a minimum monitor.
fn by_aspect(width: u32, height: u32) -> Option<Vec<AdapterScreen>> {
    if width == 0 || height == 0 {
        return None;
    }
    let aspect = f64::from(width) / f64::from(height);
    let half = |x, y, width, height| AdapterScreen {
        x,
        y,
        width,
        height,
        guess: Some(ScreenGuess::Aspect),
    };
    if aspect > WIDE_ASPECT && width / 2 >= MIN_WIDTH && height >= MIN_HEIGHT {
        let left = width / 2;
        return Some(vec![
            half(0, 0, left, height),
            half(left, 0, width - left, height),
        ]);
    }
    if (TALL_ASPECT_MIN..TALL_ASPECT).contains(&aspect)
        && width >= MIN_WIDTH
        && height / 2 >= MIN_HEIGHT
    {
        let top = height / 2;
        return Some(vec![
            half(0, 0, width, top),
            half(0, top, width, height - top),
        ]);
    }
    None
}

/// One bit per framebuffer pixel: set = pure black. Mirrors the VNC drawing
/// calls so the driver can guess without reading the shared frame.
pub struct BlackMask {
    width: u32,
    height: u32,
    words_per_row: usize,
    bits: Vec<u64>,
}

impl BlackMask {
    /// All black, like a freshly resized frame.
    pub fn new(width: u32, height: u32) -> Self {
        let words_per_row = (width as usize).div_ceil(64);
        Self {
            width,
            height,
            words_per_row,
            bits: vec![u64::MAX; words_per_row * height as usize],
        }
    }

    fn at(&self, x: u32, y: u32) -> (usize, u64) {
        (
            y as usize * self.words_per_row + x as usize / 64,
            1 << (x % 64),
        )
    }

    fn set(&mut self, x: u32, y: u32, black: bool) {
        let (word, bit) = self.at(x, y);
        if black {
            self.bits[word] |= bit;
        } else {
            self.bits[word] &= !bit;
        }
    }

    fn clip(&self, x: u32, y: u32, w: u32, h: u32) -> Option<(u32, u32)> {
        let w = w.min(self.width.saturating_sub(x));
        let h = h.min(self.height.saturating_sub(y));
        (w != 0 && h != 0).then_some((w, h))
    }

    /// BGRA8 pixels at (x, y), `stride` bytes per source row.
    pub fn put(&mut self, x: u32, y: u32, w: u32, h: u32, src: &[u8], stride: usize) {
        let Some((cw, ch)) = self.clip(x, y, w, h) else {
            return;
        };
        let Some(view) = Bgra::new(src, cw, ch, stride) else {
            return;
        };
        for row in 0..ch {
            for col in 0..cw {
                self.set(x + col, y + row, view.is_black(col, row));
            }
        }
    }

    pub fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, bgra: [u8; 4]) {
        let Some((cw, ch)) = self.clip(x, y, w, h) else {
            return;
        };
        let black = bgra[..3] == [0, 0, 0];
        for row in y..y + ch {
            for col in x..x + cw {
                self.set(col, row, black);
            }
        }
    }

    /// Copy inside the frame; source and destination may overlap.
    pub fn copy(&mut self, sx: u32, sy: u32, dx: u32, dy: u32, w: u32, h: u32) {
        let Some((w, h)) = self.clip(sx, sy, w, h) else {
            return;
        };
        let Some((w, h)) = self.clip(dx, dy, w, h) else {
            return;
        };
        let mut line = Vec::with_capacity(w as usize);
        for step in 0..h {
            let row = if dy > sy { h - 1 - step } else { step };
            line.clear();
            line.extend((0..w).map(|col| self.is_black(sx + col, sy + row)));
            for (col, &black) in (0..w).zip(&line) {
                self.set(dx + col, dy + row, black);
            }
        }
    }
}

impl Pixels for BlackMask {
    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn is_black(&self, x: u32, y: u32) -> bool {
        let (word, bit) = self.at(x, y);
        self.bits[word] & bit != 0
    }
}

/// What [`Guesser`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Not settled yet.
    Wait,
    /// Two samples at least CONFIRM_GAP apart agreed; empty = one monitor.
    Agreed(Vec<AdapterScreen>),
    /// MAX_SAMPLES samples never agreed.
    GaveUp,
}

/// Guesses the layout of one frame size from the VNC drawing calls. The
/// first sample is the first update with pixels; later ones are taken at
/// least CONFIRM_GAP after the previous, only between updates.
pub struct Guesser {
    mask: BlackMask,
    /// After resume, do not interpret an old in-flight incremental as a full frame.
    unpainted: Option<BlackMask>,
    painted: bool,
    /// Drawing since the last commit.
    drawing: bool,
    sample: Option<(Instant, Vec<AdapterScreen>)>,
    samples: u32,
}

impl Guesser {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            mask: BlackMask::new(width, height),
            unpainted: None,
            painted: false,
            drawing: false,
            sample: None,
            samples: 0,
        }
    }

    pub fn after_resume(width: u32, height: u32) -> Self {
        Self {
            unpainted: Some(BlackMask::new(width, height)),
            ..Self::new(width, height)
        }
    }

    fn painted_rect(&mut self, x: u32, y: u32, w: u32, h: u32) {
        if let Some(unpainted) = &mut self.unpainted {
            unpainted.fill(x, y, w, h, [255; 4]);
        }
    }

    pub fn put(&mut self, x: u32, y: u32, w: u32, h: u32, src: &[u8], stride: usize) {
        self.drawing = true;
        self.mask.put(x, y, w, h, src, stride);
        self.painted_rect(x, y, w, h);
    }

    pub fn fill(&mut self, x: u32, y: u32, w: u32, h: u32, bgra: [u8; 4]) {
        self.drawing = true;
        self.mask.fill(x, y, w, h, bgra);
        self.painted_rect(x, y, w, h);
    }

    pub fn copy(&mut self, sx: u32, sy: u32, dx: u32, dy: u32, w: u32, h: u32) {
        self.drawing = true;
        self.mask.copy(sx, sy, dx, dy, w, h);
    }

    /// One update applied; `painted` = it carried pixels.
    pub fn commit(&mut self, painted: bool, now: Instant) -> Verdict {
        self.drawing = false;
        if let Some(mask) = &self.unpainted {
            let tail = mask.width % 64;
            let complete = mask.bits.iter().enumerate().all(|(index, word)| {
                let valid = if tail != 0 && (index + 1) % mask.words_per_row == 0 {
                    (1u64 << tail) - 1
                } else {
                    u64::MAX
                };
                word & valid == 0
            });
            if !complete {
                return Verdict::Wait;
            }
            self.unpainted = None;
        }
        if painted && !self.painted {
            self.painted = true;
            return self.take(now);
        }
        self.poll(now)
    }

    /// Between updates: sample again once CONFIRM_GAP has passed.
    pub fn poll(&mut self, now: Instant) -> Verdict {
        match &self.sample {
            Some((at, _)) if !self.drawing && now.saturating_duration_since(*at) >= CONFIRM_GAP => {
                self.take(now)
            }
            _ => Verdict::Wait,
        }
    }

    fn take(&mut self, now: Instant) -> Verdict {
        let screens = guess(&self.mask);
        self.samples += 1;
        if self
            .sample
            .as_ref()
            .is_some_and(|(_, previous)| *previous == screens)
        {
            return Verdict::Agreed(screens);
        }
        if self.samples >= MAX_SAMPLES {
            return Verdict::GaveUp;
        }
        self.sample = Some((now, screens));
        Verdict::Wait
    }
}
