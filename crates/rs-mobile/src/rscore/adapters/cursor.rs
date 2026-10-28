//! Cursor RGBA → PNG RsEvent.

use crate::rscore::ffi_session::{
    empty_event, RsEvent, RS_EV_CURSOR_HIDDEN, RS_EV_CURSOR_POS, RS_EV_CURSOR_SHAPE,
    RS_EV_CURSOR_USE,
};

/// Encode tightly packed, non-premultiplied RGBA8 pixels as an 8-bit RGBA PNG.
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let len = (w as usize)
        .checked_mul(h as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if w == 0 || h == 0 || len != Some(rgba.len()) {
        return Err("invalid cursor dimensions or RGBA length".into());
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(rgba)
            .map_err(|error| error.to_string())?;
        writer.finish().map_err(|error| error.to_string())?;
    }
    Ok(bytes)
}

struct Shape {
    id: u64,
    width: u32,
    height: u32,
    hot_x: u32,
    hot_y: u32,
    png: Vec<u8>,
}

impl Shape {
    fn event(&self) -> RsEvent {
        let mut event = empty_event(RS_EV_CURSOR_SHAPE);
        event.shape_id = self.id;
        event.width = self.width;
        event.height = self.height;
        event.hot_x = self.hot_x;
        event.hot_y = self.hot_y;
        event.scale = 1.0;
        event.data = self.png.as_ptr();
        event.data_len = self.png.len();
        event
    }
}

#[derive(Clone, Copy)]
enum Selection {
    Use(u64),
    Hidden(bool),
}

impl Selection {
    fn event(self) -> RsEvent {
        match self {
            Self::Use(id) => {
                let mut event = empty_event(RS_EV_CURSOR_USE);
                event.shape_id = id;
                event
            }
            Self::Hidden(visible) => {
                let mut event = empty_event(RS_EV_CURSOR_HIDDEN);
                event.cursor_visible = u8::from(visible);
                event
            }
        }
    }
}

/// Owns replay data, including PNG storage. Event pointers borrow this storage
/// until the next mutation; callers must finish callbacks before mutating it.
#[derive(Default)]
pub struct CursorState {
    last_id: u64,
    shape: Option<Shape>,
    selection: Option<Selection>,
    position: Option<(f32, f32)>,
    hidden: bool,
}

impl CursorState {
    pub fn shape(&mut self, w: u32, h: u32, hot_x: u32, hot_y: u32, rgba: &[u8]) -> Vec<RsEvent> {
        if w == 0 || h == 0 {
            self.hidden = true;
            self.selection = Some(Selection::Hidden(false));
            return vec![Selection::Hidden(false).event()];
        }
        let png = match encode_png(w, h, rgba) {
            Ok(png) => png,
            Err(error) => {
                tracing::warn!(%error, "ignoring invalid cursor shape");
                return Vec::new();
            }
        };
        self.last_id = self
            .last_id
            .checked_add(1)
            .expect("cursor shape ID space exhausted");
        let shape = Shape {
            id: self.last_id,
            width: w,
            height: h,
            hot_x,
            hot_y,
            png,
        };
        let mut events = vec![shape.event(), Selection::Use(shape.id).event()];
        self.selection = Some(Selection::Use(shape.id));
        self.shape = Some(shape);
        if self.hidden {
            self.selection = Some(Selection::Hidden(true));
            events.push(Selection::Hidden(true).event());
        }
        self.hidden = false;
        events
    }

    pub fn pos(&mut self, x: u32, y: u32, fb_w: u32, fb_h: u32) -> Option<RsEvent> {
        if fb_w == 0 || fb_h == 0 {
            return None;
        }
        let position = (
            x as f32 / fb_w.saturating_sub(1).max(1) as f32,
            y as f32 / fb_h.saturating_sub(1).max(1) as f32,
        );
        self.position = Some(position);
        Some(self.position_event(position))
    }

    fn position_event(&self, (x, y): (f32, f32)) -> RsEvent {
        let mut event = empty_event(RS_EV_CURSOR_POS);
        event.cursor_x = x;
        event.cursor_y = y;
        event.cursor_visible = u8::from(!self.hidden);
        event
    }

    pub fn replay(&self) -> Vec<RsEvent> {
        let mut events = Vec::new();
        if let Some(shape) = &self.shape {
            events.push(shape.event());
        }
        if let Some(selection) = self.selection {
            events.push(selection.event());
        }
        if let Some(position) = self.position {
            // Replaying an older position must not undo a later Hidden event.
            events.push(self.position_event(position));
        }
        events
    }
}
