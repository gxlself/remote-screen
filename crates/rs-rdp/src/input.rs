//! Pointer, wheel and text input mapping.

use std::collections::BTreeMap;

use crate::scancode::hid_to_scancode;
use crate::types::{MouseButton, RdpInput};

const WHEEL_UNITS_PER_PIXEL: f64 = 1.2;
const WHEEL_UNITS_PER_LINE: f64 = 40.0;
const HWHEEL_SIGN: f64 = -1.0;
const MAX_WHEEL_UNITS: i64 = 255;
const MAX_WHEELS_PER_DIRECTION: usize = 10;

/// Stateful conversion of normalized input into RDP input events.
#[derive(Debug)]
pub struct InputMapper {
    desktop_w: u16,
    desktop_h: u16,
    pressed: BTreeMap<u16, (u8, bool)>,
    mods: u8,
    buttons: [bool; 5],
    last_position: (u16, u16),
    remainder_x: f64,
    remainder_y: f64,
}

impl InputMapper {
    /// Create a mapper for a desktop in pixels.
    pub fn new(desktop_w: u16, desktop_h: u16) -> Self {
        Self {
            desktop_w,
            desktop_h,
            pressed: BTreeMap::new(),
            mods: 0,
            buttons: [false; 5],
            last_position: (0, 0),
            remainder_x: 0.0,
            remainder_y: 0.0,
        }
    }

    /// Change the desktop dimensions used for normalized pointer coordinates.
    pub fn set_desktop_size(&mut self, w: u16, h: u16) {
        self.desktop_w = w;
        self.desktop_h = h;
    }

    /// Convert normalized coordinates to a pixel, clamping each axis first.
    pub fn to_pixels(&self, xn: f32, yn: f32) -> (u16, u16) {
        (
            normalized_to_pixel(xn, self.desktop_w),
            normalized_to_pixel(yn, self.desktop_h),
        )
    }

    /// Emit a mouse move and remember its position.
    pub fn mouse_move(&mut self, xn: f32, yn: f32) -> Vec<RdpInput> {
        let (x, y) = self.to_pixels(xn, yn);
        self.last_position = (x, y);
        vec![RdpInput::MouseMove { x, y }]
    }

    /// Emit a move and, when the button state changes, a button event.
    pub fn button(&mut self, button: u8, down: bool, xn: f32, yn: f32) -> Vec<RdpInput> {
        let Some(index) = button_index(button) else {
            return Vec::new();
        };
        let (x, y) = self.to_pixels(xn, yn);
        self.last_position = (x, y);
        let mut events = vec![RdpInput::MouseMove { x, y }];
        if self.buttons[index] == down {
            return events;
        }
        self.buttons[index] = down;
        events.push(RdpInput::MouseButton {
            button: mouse_button(index),
            down,
            x,
            y,
        });
        events
    }

    /// Convert a scroll delta into RDP wheel events, retaining fractional units.
    pub fn scroll(&mut self, dx: f32, dy: f32, line_unit: bool) -> Vec<RdpInput> {
        if !dx.is_finite() || !dy.is_finite() {
            return Vec::new();
        }

        let scale = if line_unit {
            WHEEL_UNITS_PER_LINE
        } else {
            WHEEL_UNITS_PER_PIXEL
        };
        let vertical = whole_wheel_units(f64::from(dy) * scale, &mut self.remainder_y);
        let horizontal =
            whole_wheel_units(f64::from(dx) * scale * HWHEEL_SIGN, &mut self.remainder_x);

        let mut events = Vec::new();
        append_wheels(&mut events, false, vertical);
        append_wheels(&mut events, true, horizontal);
        events
    }

    /// Discard both fractional scroll remainders.
    pub fn reset_scroll(&mut self) {
        self.remainder_x = 0.0;
        self.remainder_y = 0.0;
    }

    /// Synchronize modifiers and emit a key event for a non-modifier usage.
    pub fn key(&mut self, usage: u16, down: bool, mods: u8) -> Vec<RdpInput> {
        let mut events = Vec::new();
        if let Some(bit) = modifier_bit(usage) {
            let target = if down { mods | bit } else { mods & !bit };
            self.sync_modifiers(target, &mut events);
            return events;
        }

        self.sync_modifiers(mods, &mut events);
        let Some((scancode, extended)) = hid_to_scancode(usage) else {
            return events;
        };

        if down {
            self.pressed.entry(usage).or_insert((scancode, extended));
            events.push(RdpInput::Key {
                scancode,
                extended,
                down: true,
            });
        } else if let Some((scancode, extended)) = self.pressed.remove(&usage) {
            events.push(RdpInput::Key {
                scancode,
                extended,
                down: false,
            });
        }
        events
    }

    /// Convert text to Unicode pairs and the supported Enter/Tab scan codes.
    pub fn text(&mut self, text: &str) -> Vec<RdpInput> {
        let mut events = Vec::new();
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\r' => {
                    push_scan_pair(&mut events, 0x1C, false);
                    if chars.next_if_eq(&'\n').is_some() {
                        // CRLF is one Enter event.
                    }
                }
                '\n' => push_scan_pair(&mut events, 0x1C, false),
                '\t' => push_scan_pair(&mut events, 0x0F, false),
                ch if ch.is_control() => {}
                ch => {
                    let mut units = [0_u16; 2];
                    for code in ch.encode_utf16(&mut units).iter().copied() {
                        events.push(RdpInput::Unicode { code, down: true });
                        events.push(RdpInput::Unicode { code, down: false });
                    }
                }
            }
        }
        events
    }

    /// Release every tracked key, modifier, and button, then append ReleaseAll.
    pub fn release_all(&mut self) -> Vec<RdpInput> {
        let mut events = Vec::with_capacity(
            self.pressed.len() + self.mods.count_ones() as usize + self.buttons.len() + 1,
        );
        for &(scancode, extended) in self.pressed.values() {
            events.push(RdpInput::Key {
                scancode,
                extended,
                down: false,
            });
        }
        self.pressed.clear();

        for index in 0..8 {
            let bit = 1_u8 << index;
            if self.mods & bit == 0 {
                continue;
            }
            if let Some((scancode, extended)) = hid_to_scancode(0xE0 + index as u16) {
                events.push(RdpInput::Key {
                    scancode,
                    extended,
                    down: false,
                });
            }
        }
        self.mods = 0;

        let (x, y) = self.last_position;
        for index in 0..self.buttons.len() {
            if !self.buttons[index] {
                continue;
            }
            self.buttons[index] = false;
            events.push(RdpInput::MouseButton {
                button: mouse_button(index),
                down: false,
                x,
                y,
            });
        }

        self.reset_scroll();
        events.push(RdpInput::ReleaseAll);
        events
    }

    fn sync_modifiers(&mut self, target: u8, events: &mut Vec<RdpInput>) {
        for index in 0..8 {
            let bit = 1_u8 << index;
            let wanted = target & bit != 0;
            let held = self.mods & bit != 0;
            if wanted == held {
                continue;
            }
            if let Some((scancode, extended)) = hid_to_scancode(0xE0 + index as u16) {
                events.push(RdpInput::Key {
                    scancode,
                    extended,
                    down: wanted,
                });
            }
            if wanted {
                self.mods |= bit;
            } else {
                self.mods &= !bit;
            }
        }
    }
}

fn normalized_to_pixel(value: f32, size: u16) -> u16 {
    let normalized = if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    };
    (normalized * f32::from(size.saturating_sub(1))).round() as u16
}

fn button_index(button: u8) -> Option<usize> {
    (button <= 4).then_some(button as usize)
}

fn mouse_button(index: usize) -> MouseButton {
    match index {
        0 => MouseButton::Left,
        1 => MouseButton::Right,
        2 => MouseButton::Middle,
        3 => MouseButton::X1,
        4 => MouseButton::X2,
        _ => unreachable!("button index is limited to five entries"),
    }
}

fn modifier_bit(usage: u16) -> Option<u8> {
    if (0xE0..=0xE7).contains(&usage) {
        Some(1_u8 << (usage - 0xE0))
    } else {
        None
    }
}

fn whole_wheel_units(delta: f64, remainder: &mut f64) -> i64 {
    let sum = delta + *remainder;
    let whole = sum.trunc();
    *remainder = sum - whole;
    whole.clamp(
        -(MAX_WHEEL_UNITS * MAX_WHEELS_PER_DIRECTION as i64) as f64,
        (MAX_WHEEL_UNITS * MAX_WHEELS_PER_DIRECTION as i64) as f64,
    ) as i64
}

fn append_wheels(events: &mut Vec<RdpInput>, horizontal: bool, units: i64) {
    let mut remaining = units;
    for _ in 0..MAX_WHEELS_PER_DIRECTION {
        if remaining == 0 {
            break;
        }
        let chunk = remaining.clamp(-MAX_WHEEL_UNITS, MAX_WHEEL_UNITS);
        events.push(RdpInput::Wheel {
            horizontal,
            units: chunk as i16,
        });
        remaining -= chunk;
    }
}

fn push_scan_pair(events: &mut Vec<RdpInput>, scancode: u8, extended: bool) {
    events.push(RdpInput::Key {
        scancode,
        extended,
        down: true,
    });
    events.push(RdpInput::Key {
        scancode,
        extended,
        down: false,
    });
}
