//! 时间码识别与标定。

use crate::timecode::{
    bits_to_word, cell_center_norm, decode_word, quiet_probes_norm, Code, GrayImage, DATA_BITS,
    DATA_COLS, GRID_H, GRID_W,
};

/// 远端画面在采集图里的位置（像素，浮点）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 单帧时间码解码失败的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    OutOfBounds,
    LowContrast,
    QuietZone,
    Invalid,
}

impl DecodeError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OutOfBounds => "out-of-bounds",
            Self::LowContrast => "low-contrast",
            Self::QuietZone => "quiet-zone",
            Self::Invalid => "invalid",
        }
    }
}

/// 以 `(round(cx), round(cy))` 为中心的方形邻域均值。
///
/// 邻域与图像相交时只计算图像内的像素；若完全不相交则返回零。
pub fn sample_mean(img: &GrayImage, cx: f64, cy: f64, radius: u32) -> u8 {
    if img.width == 0 || img.height == 0 || !cx.is_finite() || !cy.is_finite() {
        return 0;
    }

    let center_x = cx.round() as i64;
    let center_y = cy.round() as i64;
    let radius = radius as i64;
    let x0 = center_x.saturating_sub(radius).clamp(0, img.width as i64) as usize;
    let y0 = center_y.saturating_sub(radius).clamp(0, img.height as i64) as usize;
    let x1 = center_x
        .saturating_add(radius)
        .saturating_add(1)
        .clamp(0, img.width as i64) as usize;
    let y1 = center_y
        .saturating_add(radius)
        .saturating_add(1)
        .clamp(0, img.height as i64) as usize;
    if x0 >= x1 || y0 >= y1 {
        return 0;
    }

    let mut sum = 0u64;
    let count = (x1 - x0) as u64 * (y1 - y0) as u64;
    for y in y0..y1 {
        let row = y * img.width as usize;
        for &value in &img.data[row + x0..row + x1] {
            sum += u64::from(value);
        }
    }
    ((sum + count / 2) / count) as u8
}

/// 解码矩形中的一个 RSTC1 时间码。
pub fn decode_at(img: &GrayImage, rect: Rect) -> Result<Code, DecodeError> {
    if !rect.x.is_finite()
        || !rect.y.is_finite()
        || !rect.w.is_finite()
        || !rect.h.is_finite()
        || rect.w <= 0.0
        || rect.h <= 0.0
    {
        return Err(DecodeError::OutOfBounds);
    }
    let cell_w = rect.w / GRID_W as f64;
    let cell_h = rect.h / GRID_H as f64;
    let sample_radius = (cell_w.min(cell_h) / 6.0).floor().max(1.0) as u32;
    let radius_f = sample_radius as f64;

    let mut points = [(0.0f64, 0.0f64); DATA_BITS + 4];
    for (index, point) in points.iter_mut().take(DATA_BITS).enumerate() {
        let col = (index as u32) % DATA_COLS;
        let row = (index as u32) / DATA_COLS;
        let (u, v) = cell_center_norm(col, row);
        *point = (rect.x + u * rect.w, rect.y + v * rect.h);
    }
    for (index, &(u, v)) in quiet_probes_norm().iter().enumerate() {
        points[DATA_BITS + index] = (rect.x + u * rect.w, rect.y + v * rect.h);
    }

    for &(px, py) in &points {
        if !px.is_finite()
            || !py.is_finite()
            || px < radius_f
            || py < radius_f
            || px + radius_f >= img.width as f64
            || py + radius_f >= img.height as f64
        {
            return Err(DecodeError::OutOfBounds);
        }
    }

    let mut values = [0u8; DATA_BITS];
    for (index, &(px, py)) in points[..DATA_BITS].iter().enumerate() {
        values[index] = sample_mean(img, px, py, sample_radius);
    }
    let mut lo = values[0] as u16;
    let mut hi = lo;
    for &value in &values[1..] {
        let value = value as u16;
        lo = lo.min(value);
        hi = hi.max(value);
    }
    let contrast = hi - lo;
    if contrast < 48 {
        return Err(DecodeError::LowContrast);
    }

    let threshold = (lo + hi) / 2;
    let mut bits = [false; DATA_BITS];
    for (index, &value) in values.iter().enumerate() {
        bits[index] = value as u16 > threshold;
    }

    let quiet_limit = lo + contrast / 3;
    for &(px, py) in &points[DATA_BITS..] {
        if sample_mean(img, px, py, sample_radius) as u16 >= quiet_limit {
            return Err(DecodeError::QuietZone);
        }
    }

    decode_word(bits_to_word(&bits)).ok_or(DecodeError::Invalid)
}

/// 找出两帧之间发生大幅亮度变化的矩形区域。
pub fn change_rect(prev: &GrayImage, cur: &GrayImage) -> Option<Rect> {
    if prev.width != cur.width || prev.height != cur.height {
        return None;
    }
    let sw = (cur.width as usize).div_ceil(2);
    let sh = (cur.height as usize).div_ceil(2);
    let total = sw.checked_mul(sh)?;
    if total == 0 {
        return None;
    }

    let mut changed = vec![false; total];
    let mut row_counts = vec![0usize; sh];
    let mut changed_count = 0usize;
    for (gy, row_count) in row_counts.iter_mut().enumerate() {
        let y = gy * 2;
        for gx in 0..sw {
            let x = gx * 2;
            let old = prev.get(x as u32, y as u32);
            let new = cur.get(x as u32, y as u32);
            let different = old.abs_diff(new) >= 96;
            changed[gy * sw + gx] = different;
            *row_count += usize::from(different);
        }
        changed_count += *row_count;
    }
    if changed_count < total.div_ceil(50) {
        return None;
    }

    let (row_start, row_end) = longest_segment(&row_counts)?;

    let mut col_counts = vec![0usize; sw];
    for gy in row_start..=row_end {
        for (gx, count) in col_counts.iter_mut().enumerate() {
            *count += usize::from(changed[gy * sw + gx]);
        }
    }
    let (col_start, col_end) = longest_segment(&col_counts)?;

    if row_end - row_start + 1 < 8 || col_end - col_start + 1 < 8 {
        return None;
    }

    Some(Rect {
        x: (col_start * 2) as f64,
        y: (row_start * 2) as f64,
        w: ((col_end - col_start + 1) * 2) as f64,
        h: ((row_end - row_start + 1) * 2) as f64,
    })
}

// Only qualifying indices become endpoints; short internal gaps are included.
fn longest_segment(counts: &[usize]) -> Option<(usize, usize)> {
    let cutoff = counts.iter().max()? / 2;
    let gap_limit = 2usize.max(counts.len() / 100);
    let mut best = None;
    let mut best_len = 0usize;
    let mut start = None;
    let mut last_good = 0usize;

    for (index, &count) in counts.iter().enumerate() {
        if count < cutoff {
            continue;
        }
        let segment_start = match start {
            Some(start) if index - last_good - 1 <= gap_limit => start,
            _ => index,
        };
        start = Some(segment_start);
        last_good = index;
        let segment_len = index - segment_start + 1;
        if segment_len > best_len {
            best = Some((segment_start, index));
            best_len = segment_len;
        }
    }
    best
}

#[derive(Debug, Clone, Copy)]
pub struct TrackerConfig {
    pub aspect_tol: f64,
    pub stable_tol: f64,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            aspect_tol: 0.03,
            stable_tol: 0.01,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrackEvent {
    Searching,
    Calibrating,
    Locked {
        rect: Rect,
        host_w: u32,
        host_h: u32,
    },
    Calib,
    Time(u64),
    End,
    Fail(DecodeError),
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    rect: Rect,
    host_w: u32,
    host_h: u32,
}

/// 标定成功后保持锁定；后续错误不会触发重新标定。
pub struct Tracker {
    cfg: TrackerConfig,
    prev: Option<GrayImage>,
    candidate: Option<Candidate>,
    locked: Option<(Rect, u32, u32)>,
    locked_size: Option<(u32, u32)>,
}

impl Tracker {
    pub fn new(cfg: TrackerConfig) -> Self {
        Self {
            cfg,
            prev: None,
            candidate: None,
            locked: None,
            locked_size: None,
        }
    }

    pub fn push(&mut self, img: &GrayImage) -> TrackEvent {
        if let Some((rect, _, _)) = self.locked {
            if self.locked_size != Some((img.width, img.height)) {
                return TrackEvent::Fail(DecodeError::OutOfBounds);
            }
            return match decode_at(img, rect) {
                Ok(Code::Calib { .. }) => TrackEvent::Calib,
                Ok(Code::Time(value)) => TrackEvent::Time(value),
                Ok(Code::End) => TrackEvent::End,
                Err(error) => TrackEvent::Fail(error),
            };
        }

        let Some(previous) = self.prev.as_ref() else {
            self.prev = Some(img.clone());
            return TrackEvent::Searching;
        };
        let rect = change_rect(previous, img);
        self.prev = Some(img.clone());
        let Some(rect) = rect else {
            return TrackEvent::Searching;
        };

        let Ok(Code::Calib { width, height }) = decode_at(img, rect) else {
            return TrackEvent::Searching;
        };
        let image_ratio = rect.w / rect.h;
        let host_ratio = width as f64 / height as f64;
        if !image_ratio.is_finite()
            || !host_ratio.is_finite()
            || (image_ratio / host_ratio - 1.0).abs() > self.cfg.aspect_tol
        {
            return TrackEvent::Searching;
        }

        let next = Candidate {
            rect,
            host_w: width,
            host_h: height,
        };
        if let Some(old) = self.candidate {
            let tolerance = self.cfg.stable_tol * rect.w.max(rect.h);
            if old.host_w == width
                && old.host_h == height
                && (old.rect.x - rect.x).abs() <= tolerance
                && (old.rect.y - rect.y).abs() <= tolerance
                && (old.rect.w - rect.w).abs() <= tolerance
                && (old.rect.h - rect.h).abs() <= tolerance
            {
                let locked_rect = Rect {
                    x: (old.rect.x + rect.x) / 2.0,
                    y: (old.rect.y + rect.y) / 2.0,
                    w: (old.rect.w + rect.w) / 2.0,
                    h: (old.rect.h + rect.h) / 2.0,
                };
                self.locked = Some((locked_rect, width, height));
                self.locked_size = Some((img.width, img.height));
                self.prev = None;
                self.candidate = None;
                return TrackEvent::Locked {
                    rect: locked_rect,
                    host_w: width,
                    host_h: height,
                };
            }
        }
        self.candidate = Some(next);
        TrackEvent::Calibrating
    }

    pub fn locked(&self) -> Option<(Rect, u32, u32)> {
        self.locked
    }
}
