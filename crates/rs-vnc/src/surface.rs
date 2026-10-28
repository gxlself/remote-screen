//! Decode target . Pixels are always BGRA8 (B, G, R, A=255).
//! Cards must not change this file.

use crate::types::Rect;

/// One monitor of an ExtendedDesktopSize layout; `rect` is in framebuffer pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    pub id: u32,
    pub rect: Rect,
    pub flags: u32,
}

pub trait Surface: Send {
    /// New framebuffer size (ServerInit, DesktopSize). Contents become black.
    fn resize(&mut self, w: u16, h: u16);
    fn size(&self) -> (u16, u16);
    /// Write BGRA8 pixels; `src` rows are `stride` bytes apart. Callers have
    /// checked `r.fits(size)`; implementations may still clip.
    fn put_bgra(&mut self, r: Rect, src: &[u8], stride: usize);
    fn fill(&mut self, r: Rect, bgra: [u8; 4]);
    /// CopyRect: copy the `r.w` x `r.h` block at (sx, sy) to `r`; may overlap.
    fn copy_rect(&mut self, sx: u16, sy: u16, r: Rect);
    /// One FramebufferUpdate fully applied; `dirty` lists the rectangles written.
    fn commit(&mut self, dirty: &[Rect]);
    /// Cursor shape as BGRA8 with A = 0 or 255 (mask applied). w == 0 || h == 0 hides it.
    fn cursor(&mut self, w: u16, h: u16, hot_x: u16, hot_y: u16, bgra: &[u8]);
    /// PointerPos pseudo-encoding.
    fn pointer_pos(&mut self, x: u16, y: u16);
    /// ExtendedDesktopSize monitor layout in server order . Only
    /// non-empty layouts whose screens all fit the framebuffer arrive here.
    fn screens(&mut self, screens: &[Screen]) {
        let _ = screens;
    }
}

/// Whole-framebuffer memory surface for tests and as a reference implementation.
#[derive(Debug, Default, Clone)]
pub struct MemSurface {
    pub w: u16,
    pub h: u16,
    pub pixels: Vec<u8>,
    pub commits: Vec<Vec<Rect>>,
    /// (w, h, hot_x, hot_y, bgra)
    pub cursor: Option<(u16, u16, u16, u16, Vec<u8>)>,
    pub pointer: Option<(u16, u16)>,
    /// The last reported monitor layout.
    pub screens: Option<Vec<Screen>>,
}

impl MemSurface {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pixel(&self, x: u16, y: u16) -> [u8; 4] {
        let i = (usize::from(y) * usize::from(self.w) + usize::from(x)) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    fn clip(&self, r: Rect) -> Option<(usize, usize, usize, usize)> {
        let x0 = usize::from(r.x).min(usize::from(self.w));
        let y0 = usize::from(r.y).min(usize::from(self.h));
        let x1 = (usize::from(r.x) + usize::from(r.w)).min(usize::from(self.w));
        let y1 = (usize::from(r.y) + usize::from(r.h)).min(usize::from(self.h));
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
    }
}

impl Surface for MemSurface {
    fn resize(&mut self, w: u16, h: u16) {
        self.w = w;
        self.h = h;
        self.pixels = [0u8, 0, 0, 255].repeat(usize::from(w) * usize::from(h));
    }

    fn size(&self) -> (u16, u16) {
        (self.w, self.h)
    }

    fn put_bgra(&mut self, r: Rect, src: &[u8], stride: usize) {
        let Some((x0, y0, x1, y1)) = self.clip(r) else {
            return;
        };
        let row_bytes = (x1 - x0) * 4;
        for y in y0..y1 {
            let s = (y - usize::from(r.y)) * stride + (x0 - usize::from(r.x)) * 4;
            let d = (y * usize::from(self.w) + x0) * 4;
            self.pixels[d..d + row_bytes].copy_from_slice(&src[s..s + row_bytes]);
        }
    }

    fn fill(&mut self, r: Rect, bgra: [u8; 4]) {
        let Some((x0, y0, x1, y1)) = self.clip(r) else {
            return;
        };
        for y in y0..y1 {
            for x in x0..x1 {
                let d = (y * usize::from(self.w) + x) * 4;
                self.pixels[d..d + 4].copy_from_slice(&bgra);
            }
        }
    }

    fn copy_rect(&mut self, sx: u16, sy: u16, r: Rect) {
        let src_rect = Rect::new(sx, sy, r.w, r.h);
        if !src_rect.fits(self.w, self.h) || !r.fits(self.w, self.h) {
            return;
        }
        let stride = usize::from(r.w) * 4;
        let mut block = vec![0u8; stride * usize::from(r.h)];
        for row in 0..usize::from(r.h) {
            let s = ((usize::from(sy) + row) * usize::from(self.w) + usize::from(sx)) * 4;
            block[row * stride..(row + 1) * stride].copy_from_slice(&self.pixels[s..s + stride]);
        }
        self.put_bgra(r, &block, stride);
    }

    fn commit(&mut self, dirty: &[Rect]) {
        self.commits.push(dirty.to_vec());
    }

    fn cursor(&mut self, w: u16, h: u16, hot_x: u16, hot_y: u16, bgra: &[u8]) {
        self.cursor = Some((w, h, hot_x, hot_y, bgra.to_vec()));
    }

    fn pointer_pos(&mut self, x: u16, y: u16) {
        self.pointer = Some((x, y));
    }

    fn screens(&mut self, screens: &[Screen]) {
        self.screens = Some(screens.to_vec());
    }
}
