//! HID usage / text to X11 keysym, pointer mask and scroll.
//!
//! This module only computes RFB `KeyEvent` / `PointerEvent` parameters. It
//! never touches the network. Keysyms follow RFC 6143 §7.5.4 and X11's
//! `keysymdef.h`; pointer buttons 1–8 are mask bits 0–7 (RFC 6143 §7.5.5).
//!
//! Scroll input is already in the App's natural-scroll convention: `dy > 0`
//! is a wheel delta in Windows' positive direction (up), matching
//! `host-core/src/inject/win_plan.rs` (`dy * 1.2` without sign inversion and
//! horizontal input inverted).

use std::collections::BTreeMap;

const SUPER_L: u32 = 0xffeb;
const MOD_LSHIFT: u8 = 1 << 1;
const MOD_RSHIFT: u8 = 1 << 5;

const KEY_BACKSPACE: u32 = 0xff08;
const KEY_TAB: u32 = 0xff09;
const KEY_RETURN: u32 = 0xff0d;
const KEY_ESCAPE: u32 = 0xff1b;

const MODIFIER_KEYSYMS: [u32; 8] = [
    0xffe3,  // Control_L
    0xffe1,  // Shift_L
    0xffe9,  // Alt_L
    SUPER_L, // Command / Win -> Super_L
    0xffe4,  // Control_R
    0xffe2,  // Shift_R
    0xffea,  // Alt_R
    0xffec,  // Super_R
];

const MODIFIER_USAGES: [u16; 8] = [0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7];

const PIXELS_PER_SCROLL_STEP: f32 = 10.0;
const MAX_SCROLL_STEPS_PER_CALL: u32 = 10;

/// Convert a USB HID usage code on a US layout to an X11 keysym.
///
/// `shift` only affects printable keys. Returns `None` when the usage is not
/// in the supported table.
pub fn hid_to_keysym(usage: u16, shift: bool) -> Option<u32> {
    match usage {
        // a–z
        0x04..=0x1D => {
            let offset = u32::from(usage - 0x04);
            Some(if shift { 0x41 + offset } else { 0x61 + offset })
        }
        // 1–9
        0x1E..=0x26 => {
            const UNSHIFTED: [u32; 9] = [0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39];
            const SHIFTED: [u32; 9] = [0x21, 0x40, 0x23, 0x24, 0x25, 0x5e, 0x26, 0x2a, 0x28];
            let index = usize::from(usage - 0x1E);
            Some(if shift {
                SHIFTED[index]
            } else {
                UNSHIFTED[index]
            })
        }
        // 0
        0x27 => Some(if shift { 0x29 } else { 0x30 }),
        0x28 => Some(KEY_RETURN),
        0x29 => Some(KEY_ESCAPE),
        0x2A => Some(KEY_BACKSPACE),
        0x2B => Some(KEY_TAB),
        0x2C => Some(0x20),
        0x2D => Some(if shift { 0x5f } else { 0x2d }),
        0x2E => Some(if shift { 0x2b } else { 0x3d }),
        0x2F => Some(if shift { 0x7b } else { 0x5b }),
        0x30 => Some(if shift { 0x7d } else { 0x5d }),
        0x31 => Some(if shift { 0x7c } else { 0x5c }),
        0x32 => Some(if shift { 0x7e } else { 0x23 }),
        0x33 => Some(if shift { 0x3a } else { 0x3b }),
        0x34 => Some(if shift { 0x22 } else { 0x27 }),
        0x35 => Some(if shift { 0x7e } else { 0x60 }),
        0x36 => Some(if shift { 0x3c } else { 0x2c }),
        0x37 => Some(if shift { 0x3e } else { 0x2e }),
        0x38 => Some(if shift { 0x3f } else { 0x2f }),
        0x39 => Some(0xffe5),
        0x3A..=0x45 => Some(0xffbe + u32::from(usage - 0x3A)),
        0x46 => Some(0xff61),
        0x47 => Some(0xff14),
        0x48 => Some(0xff13),
        0x49 => Some(0xff63),
        0x4A => Some(0xff50),
        0x4B => Some(0xff55),
        0x4C => Some(0xffff),
        0x4D => Some(0xff57),
        0x4E => Some(0xff56),
        0x4F => Some(0xff53),
        0x50 => Some(0xff51),
        0x51 => Some(0xff54),
        0x52 => Some(0xff52),
        0x53 => Some(0xff7f),
        0x54 => Some(0xffaf),
        0x55 => Some(0xffaa),
        0x56 => Some(0xffad),
        0x57 => Some(0xffab),
        0x58 => Some(0xff8d),
        0x59..=0x61 => Some(0xffb1 + u32::from(usage - 0x59)),
        0x62 => Some(0xffb0),
        0x63 => Some(0xffae),
        0x64 => Some(if shift { 0x3e } else { 0x3c }),
        0x65 => Some(0xff67),
        0x67 => Some(0xffbd),
        0x68..=0x73 => Some(0xffca + u32::from(usage - 0x68)),
        0xE0 => Some(0xffe3),
        0xE1 => Some(0xffe1),
        0xE2 => Some(0xffe9),
        0xE3 => Some(SUPER_L),
        0xE4 => Some(0xffe4),
        0xE5 => Some(0xffe2),
        0xE6 => Some(0xffea),
        0xE7 => Some(0xffec),
        _ => None,
    }
}

/// Convert one Unicode scalar to an X11 keysym.
///
/// `'\n'` and `'\r'` become Return, `'\t'` becomes Tab, and U+0008 becomes
/// BackSpace. Other C0 controls and DEL / C1 controls are unsupported.
/// Latin-1 maps directly; larger code points use the Unicode keysym range
/// (`0x0100_0000 | code_point`).
pub fn char_to_keysym(c: char) -> Option<u32> {
    match c {
        '\n' | '\r' => Some(KEY_RETURN),
        '\t' => Some(KEY_TAB),
        '\u{8}' => Some(KEY_BACKSPACE),
        '\0'..='\u{1f}' | '\u{7f}'..='\u{9f}' => None,
        '\u{20}'..='\u{7e}' | '\u{a0}'..='\u{ff}' => Some(u32::from(c)),
        _ => Some(0x0100_0000 | u32::from(c)),
    }
}

/// Expand text to down/up keysym pairs, skipping unmappable characters.
///
/// CRLF is treated as a single Return, so it produces one down/up pair.
pub fn text_events(text: &str) -> Vec<(bool, u32)> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' && chars.peek() == Some(&'\n') {
            chars.next();
        }
        if let Some(keysym) = char_to_keysym(c) {
            out.push((true, keysym));
            out.push((false, keysym));
        }
    }
    out
}

/// Return the press/release sequence for Ctrl+Alt+Delete.
pub fn ctrl_alt_del() -> Vec<(bool, u32)> {
    vec![
        (true, 0xffe3),
        (true, 0xffe9),
        (true, 0xffff),
        (false, 0xffff),
        (false, 0xffe9),
        (false, 0xffe3),
    ]
}

/// Tracks held keys and modifier state and turns HID input into keysym events.
#[derive(Debug, Default)]
pub struct KeyTracker {
    /// Held ordinary usages mapped to the keysym sent on their press.
    pressed: BTreeMap<u16, u32>,
    /// Currently held modifier bits.
    mods: u8,
}

impl KeyTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Process one key transition and return the `(down, keysym)` events to send.
    ///
    /// Modifier usages (`0xE0`–`0xE7`) update and emit only the corresponding
    /// modifier bit; the `mods` argument is ignored for those usages. Ordinary
    /// keys synchronize modifiers first, then use the resulting Shift state to
    /// select the keysym.
    pub fn key(&mut self, usage: u16, down: bool, mods: u8) -> Vec<(bool, u32)> {
        let mut out = Vec::new();

        if let Some(index) = MODIFIER_USAGES
            .iter()
            .position(|&modifier_usage| modifier_usage == usage)
        {
            let mut target = mods;
            let bit = 1 << index;
            if down {
                target |= bit;
            } else {
                target &= !bit;
            }
            self.sync_mods(target, &mut out);
            return out;
        }

        self.sync_mods(mods, &mut out);

        if down {
            if let Some(keysym) = self.pressed.get(&usage) {
                out.push((true, *keysym));
            } else {
                let shift = self.mods & (MOD_LSHIFT | MOD_RSHIFT) != 0;
                if let Some(keysym) = hid_to_keysym(usage, shift) {
                    self.pressed.insert(usage, keysym);
                    out.push((true, keysym));
                }
            }
        } else if let Some(keysym) = self.pressed.remove(&usage) {
            out.push((false, keysym));
        }

        out
    }

    /// Release held ordinary keys in ascending usage order, then modifiers in
    /// bit order, and clear all tracked state.
    pub fn release_all(&mut self) -> Vec<(bool, u32)> {
        let mut out = Vec::with_capacity(self.pressed.len() + 8);
        for keysym in self.pressed.values() {
            out.push((false, *keysym));
        }
        self.pressed.clear();

        for (index, keysym) in MODIFIER_KEYSYMS.iter().enumerate() {
            if self.mods & (1 << index) != 0 {
                out.push((false, *keysym));
            }
        }
        self.mods = 0;
        out
    }

    fn sync_mods(&mut self, target: u8, out: &mut Vec<(bool, u32)>) {
        for (index, keysym) in MODIFIER_KEYSYMS.iter().enumerate() {
            let bit = 1 << index;
            let was_down = self.mods & bit != 0;
            let wants_down = target & bit != 0;
            if was_down != wants_down {
                out.push((wants_down, *keysym));
            }
        }
        self.mods = target;
    }
}

/// Convert normalized coordinates to framebuffer pixels.
///
/// Coordinates are clamped to `[0, 1]` (NaN becomes 0) before rounding to the
/// nearest pixel. If either framebuffer dimension is zero, both coordinates
/// are 0.
pub fn to_pixels(xn: f32, yn: f32, fb_w: u16, fb_h: u16) -> (u16, u16) {
    if fb_w == 0 || fb_h == 0 {
        return (0, 0);
    }
    let x = (normalize_coordinate(xn) * f32::from(fb_w - 1)).round() as u16;
    let y = (normalize_coordinate(yn) * f32::from(fb_h - 1)).round() as u16;
    (x, y)
}

fn normalize_coordinate(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

/// Tracks the RFB pointer mask, last pointer position, and scroll remainders.
#[derive(Debug, Default)]
pub struct PointerState {
    mask: u8,
    x: u16,
    y: u16,
    scroll_x_remainder: f64,
    scroll_y_remainder: f64,
}

impl PointerState {
    /// Create an empty pointer state at `(0, 0)` with no buttons held.
    pub fn new() -> Self {
        Self::default()
    }

    /// Move to normalized coordinates and return the RFB pointer state.
    pub fn move_to(&mut self, xn: f32, yn: f32, fb_w: u16, fb_h: u16) -> (u8, u16, u16) {
        let (x, y) = to_pixels(xn, yn, fb_w, fb_h);
        self.x = x;
        self.y = y;
        (self.mask, x, y)
    }

    /// Apply a button transition at the given position.
    ///
    /// Buttons 0, 1 and 2 map to the left, right and middle RFB mask bits.
    /// Back and forward buttons are unsupported and produce no events.
    pub fn button(
        &mut self,
        button: u8,
        down: bool,
        xn: f32,
        yn: f32,
        fb_w: u16,
        fb_h: u16,
    ) -> Vec<(u8, u16, u16)> {
        let Some(bit) = button_mask(button) else {
            return Vec::new();
        };
        let (_, x, y) = self.move_to(xn, yn, fb_w, fb_h);
        if down {
            self.mask |= bit;
        } else {
            self.mask &= !bit;
        }
        vec![(self.mask, x, y)]
    }

    /// Convert a scroll delta into wheel button press/release pairs.
    ///
    /// `line_unit == false` treats deltas as pixels (10 px per step);
    /// otherwise each unit is one line. Positive `dy` maps to button 4, negative
    /// `dy` to button 5, positive `dx` to button 6, and negative `dx` to button
    /// 7. Each direction emits at most 10 steps per call; excess input is
    /// dropped, while sub-step input remains for the next call. Non-finite
    /// deltas leave both the result and the remainders unchanged.
    pub fn scroll(&mut self, dx: f32, dy: f32, line_unit: bool) -> Vec<(u8, u16, u16)> {
        if !dx.is_finite() || !dy.is_finite() {
            return Vec::new();
        }

        let mut out = Vec::new();
        let vertical_steps = scroll_steps(dy, line_unit, &mut self.scroll_y_remainder);
        let horizontal_steps = scroll_steps(dx, line_unit, &mut self.scroll_x_remainder);

        let vertical_bit = if vertical_steps >= 0 { 1 << 3 } else { 1 << 4 };
        append_scroll_events(
            &mut out,
            vertical_steps,
            vertical_bit,
            self.mask,
            self.x,
            self.y,
        );

        let horizontal_bit = if horizontal_steps >= 0 {
            1 << 5
        } else {
            1 << 6
        };
        append_scroll_events(
            &mut out,
            horizontal_steps,
            horizontal_bit,
            self.mask,
            self.x,
            self.y,
        );

        out
    }

    /// Clear accumulated scroll remainders at the start of a new gesture.
    pub fn reset_scroll(&mut self) {
        self.scroll_x_remainder = 0.0;
        self.scroll_y_remainder = 0.0;
    }

    /// Release all pointer buttons and clear scroll remainders.
    ///
    /// Returns the release pointer event only when a button was held.
    pub fn release_all(&mut self) -> Option<(u8, u16, u16)> {
        self.reset_scroll();
        if self.mask == 0 {
            return None;
        }
        self.mask = 0;
        Some((0, self.x, self.y))
    }
}

fn button_mask(button: u8) -> Option<u8> {
    match button {
        0 => Some(1 << 0),
        1 => Some(1 << 2),
        2 => Some(1 << 1),
        _ => None,
    }
}

fn scroll_steps(delta: f32, line_unit: bool, remainder: &mut f64) -> i32 {
    let scale = if line_unit {
        1.0
    } else {
        1.0 / f64::from(PIXELS_PER_SCROLL_STEP)
    };
    let total = f64::from(delta) * scale + *remainder;
    let whole = total.trunc();
    *remainder = total - whole;
    whole.clamp(
        -f64::from(MAX_SCROLL_STEPS_PER_CALL),
        f64::from(MAX_SCROLL_STEPS_PER_CALL),
    ) as i32
}

fn append_scroll_events(
    out: &mut Vec<(u8, u16, u16)>,
    steps: i32,
    bit: u8,
    mask: u8,
    x: u16,
    y: u16,
) {
    for _ in 0..steps.unsigned_abs() {
        out.push((mask | bit, x, y));
        out.push((mask, x, y));
    }
}
