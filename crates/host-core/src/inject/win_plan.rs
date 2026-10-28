//! Platform-independent planning for Windows `SendInput` injection.
//!
//! This module translates RSP input into the operations the Windows platform
//! layer must submit. It deliberately does not reference any Win32 types, so
//! the state machine can be exercised on Linux.

use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit};
use rsp_core::input::InputEvent;

use crate::inject::keymap_win::{hid_to_win, modifier_bit, WinKey};
use crate::inject::text_plan::{plan_text, TextStep, VK_RETURN, VK_TAB};

/// `dwExtraInfo` marker attached to every event injected by Remote Screen.
pub const RS_EXTRA_INFO: usize = 0x5253;
/// Windows wheel delta for one detent.
pub const WHEEL_DELTA: i32 = 120;
/// Lines crossed by one wheel detent under the Windows default setting.
pub const WHEEL_UNITS_PER_LINE: f64 = 40.0;
/// Default pixel-to-wheel-unit scale for a 100% scaled Chromium window.
pub const DEFAULT_WHEEL_UNITS_PER_PIXEL: f64 = 1.2;
/// Hardware-wheel direction for a finger moving right.
pub const HWHEEL_SIGN: i32 = -1;

const MAX_PRESSED: usize = 32;

const MODIFIERS: [(Mods, u16); 8] = [
    (Mods::LCTRL, 0xE0),
    (Mods::LSHIFT, 0xE1),
    (Mods::LALT, 0xE2),
    (Mods::LMETA, 0xE3),
    (Mods::RCTRL, 0xE4),
    (Mods::RSHIFT, 0xE5),
    (Mods::RALT, 0xE6),
    (Mods::RMETA, 0xE7),
];

const BUTTON_ORDER: [(Button, WinButton); 5] = [
    (Button::Left, WinButton::Left),
    (Button::Right, WinButton::Right),
    (Button::Middle, WinButton::Middle),
    (Button::Back, WinButton::X1),
    (Button::Forward, WinButton::X2),
];

/// Snapshot of the input state currently held by a planner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeldInput {
    /// Ordinary HID usages, in the order they were pressed.
    pub keys: Vec<u16>,
    /// Side-specific modifiers currently held by the planner.
    pub mods: Mods,
    /// Mouse buttons currently held, in protocol button order.
    pub buttons: Vec<Button>,
}

impl HeldInput {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.mods == Mods::NONE && self.buttons.is_empty()
    }
}

/// Return modifier usages in the order used when releasing them.
pub fn modifier_usages(mods: Mods) -> Vec<u16> {
    MODIFIERS
        .iter()
        .rev()
        .filter_map(|(modifier, usage)| mods.contains(*modifier).then_some(*usage))
        .collect()
}

/// Mouse buttons understood by the Windows planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// One platform-independent operation for the Windows injector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinInputOp {
    Key { key: WinKey, up: bool },
    Unicode { unit: u16, up: bool },
    MoveAbs { x: u16, y: u16 },
    Button { button: WinButton, up: bool },
    Wheel { delta: i32 },
    HWheel { delta: i32 },
}

/// Physical-pixel rectangle in Windows screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

/// Counters for input rejected or normalized by the planner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WinInputStats {
    pub unknown_usage: u64,
    pub stray_up: u64,
    pub overflow: u64,
    pub dropped: u64,
}

/// Stateful RSP-to-Windows input planner.
#[derive(Debug)]
pub struct WinPlanner {
    wheel_units_per_pixel: f64,
    pressed: Vec<(u16, WinKey)>,
    mods: Mods,
    buttons: [bool; 5],
    remainder_x: f64,
    remainder_y: f64,
    stats: WinInputStats,
    adopting: bool,
    adopted_ups: u64,
}

impl Default for WinPlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl WinPlanner {
    /// Create a planner with the documented default pixel scale.
    pub fn new() -> Self {
        Self::with_wheel_units_per_pixel(DEFAULT_WHEEL_UNITS_PER_PIXEL)
    }

    /// Create a planner with a custom pixel scale.
    ///
    /// Non-positive and non-finite values fall back to
    /// [`DEFAULT_WHEEL_UNITS_PER_PIXEL`].
    pub fn with_wheel_units_per_pixel(units: f64) -> Self {
        Self {
            wheel_units_per_pixel: if units.is_finite() && units > 0.0 {
                units
            } else {
                DEFAULT_WHEEL_UNITS_PER_PIXEL
            },
            pressed: Vec::new(),
            mods: Mods::NONE,
            buttons: [false; 5],
            remainder_x: 0.0,
            remainder_y: 0.0,
            stats: WinInputStats::default(),
            adopting: false,
            adopted_ups: 0,
        }
    }

    /// Create a planner for a newly connected session process.
    ///
    /// Until the first key/button press or text event, unmatched key and
    /// button releases are forwarded once so state left behind by the previous
    /// process can be cleared.
    pub fn for_new_process() -> Self {
        let mut planner = Self::new();
        planner.adopting = true;
        planner
    }

    /// Return the state currently tracked as held by this planner.
    pub fn held(&self) -> HeldInput {
        HeldInput {
            keys: self.pressed.iter().map(|&(usage, _)| usage).collect(),
            mods: self.mods,
            buttons: BUTTON_ORDER
                .iter()
                .enumerate()
                .filter_map(|(index, (button, _))| self.buttons[index].then_some(*button))
                .collect(),
        }
    }

    /// Convert one RSP input message into Windows injection operations.
    ///
    /// `monitor` is the display the normalized coordinates refer to; `desk` is
    /// the union of all displays. Both use physical Windows pixels.
    pub fn plan(
        &mut self,
        msg: &InputMsg,
        monitor: ScreenRect,
        desk: ScreenRect,
    ) -> Vec<WinInputOp> {
        let adopting = self.adopting;
        if adopting
            && matches!(
                msg,
                InputMsg::Key { down: true, .. }
                    | InputMsg::MouseButton { down: true, .. }
                    | InputMsg::Text(_)
            )
        {
            self.adopting = false;
        }

        match msg {
            InputMsg::MouseMove { x, y } => {
                vec![self.move_abs(*x, *y, monitor, desk)]
            }
            InputMsg::MouseButton {
                button, down, x, y, ..
            } if adopting && !down => self.plan_adopted_button_up(*button, *x, *y, monitor, desk),
            InputMsg::MouseButton {
                button, down, x, y, ..
            } => {
                let mut ops = vec![self.move_abs(*x, *y, monitor, desk)];
                let (index, win_button) = button_parts(*button);
                if *down {
                    self.buttons[index] = true;
                    ops.push(WinInputOp::Button {
                        button: win_button,
                        up: false,
                    });
                } else if self.buttons[index] {
                    self.buttons[index] = false;
                    ops.push(WinInputOp::Button {
                        button: win_button,
                        up: true,
                    });
                } else {
                    self.stats.stray_up += 1;
                }
                ops
            }
            InputMsg::Scroll { .. } => self.plan_scroll(msg),
            InputMsg::Key {
                usage, down: false, ..
            } if adopting => self.plan_adopted_key_up(*usage),
            InputMsg::Key { usage, down, mods } => self.plan_key(*usage, *down, *mods),
            InputMsg::Text(text) => self.plan_text(text),
            InputMsg::ReleaseAll => self.release_all(),
            InputMsg::Touch { .. } => {
                self.stats.dropped += 1;
                Vec::new()
            }
        }
    }

    /// Release every held ordinary key, modifier and mouse button.
    pub fn release_all(&mut self) -> Vec<WinInputOp> {
        let mut ops = Vec::with_capacity(self.pressed.len() + MODIFIERS.len() + BUTTON_ORDER.len());

        for &(_, key) in self.pressed.iter().rev() {
            ops.push(WinInputOp::Key { key, up: true });
        }
        self.pressed.clear();

        for &(modifier, usage) in MODIFIERS.iter().rev() {
            if !self.mods.contains(modifier) {
                continue;
            }
            if let Some(key) = hid_to_win(usage) {
                ops.push(WinInputOp::Key { key, up: true });
            }
        }
        self.mods = Mods::NONE;

        for (index, (_, button)) in BUTTON_ORDER.iter().enumerate() {
            if self.buttons[index] {
                self.buttons[index] = false;
                ops.push(WinInputOp::Button {
                    button: *button,
                    up: true,
                });
            }
        }

        self.remainder_x = 0.0;
        self.remainder_y = 0.0;
        ops
    }

    /// Counters accumulated since this planner was created.
    pub fn stats(&self) -> WinInputStats {
        self.stats
    }

    /// Number of unmatched releases forwarded while adopting a new process.
    pub fn adopted_ups(&self) -> u64 {
        self.adopted_ups
    }

    fn plan_adopted_key_up(&mut self, usage: u16) -> Vec<WinInputOp> {
        if let Some(modifier) = modifier_bit(usage) {
            if self.mods.contains(modifier) {
                return self.plan_key(usage, false, self.mods);
            }
        } else if self
            .pressed
            .iter()
            .any(|&(pressed_usage, _)| pressed_usage == usage)
        {
            return self.plan_key(usage, false, self.mods);
        }

        let Some(key) = hid_to_win(usage) else {
            self.stats.unknown_usage += 1;
            return Vec::new();
        };
        self.adopted_ups += 1;
        vec![WinInputOp::Key { key, up: true }]
    }

    fn plan_adopted_button_up(
        &mut self,
        button: Button,
        x: f32,
        y: f32,
        monitor: ScreenRect,
        desk: ScreenRect,
    ) -> Vec<WinInputOp> {
        let mut ops = vec![self.move_abs(x, y, monitor, desk)];
        let (index, win_button) = button_parts(button);
        if self.buttons[index] {
            self.buttons[index] = false;
        } else {
            self.adopted_ups += 1;
        }
        ops.push(WinInputOp::Button {
            button: win_button,
            up: true,
        });
        ops
    }

    fn plan_key(&mut self, usage: u16, down: bool, mods: Mods) -> Vec<WinInputOp> {
        if let Some(own_bit) = modifier_bit(usage) {
            let target = if down {
                mods | own_bit
            } else {
                Mods::from_bits(mods.bits() & !own_bit.bits())
            };
            let mut ops = Vec::with_capacity(MODIFIERS.len());
            self.sync_modifiers(target, &mut ops);
            return ops;
        }

        let Some(key) = hid_to_win(usage) else {
            self.stats.unknown_usage += 1;
            return Vec::new();
        };

        let mut ops = Vec::with_capacity(MODIFIERS.len() + 1);
        self.sync_modifiers(mods, &mut ops);
        if down {
            if self
                .pressed
                .iter()
                .any(|&(pressed_usage, _)| pressed_usage == usage)
            {
                ops.push(WinInputOp::Key { key, up: false });
            } else if self.pressed.len() >= MAX_PRESSED {
                self.stats.overflow += 1;
            } else {
                self.pressed.push((usage, key));
                ops.push(WinInputOp::Key { key, up: false });
            }
        } else if let Some(index) = self
            .pressed
            .iter()
            .position(|&(pressed_usage, _)| pressed_usage == usage)
        {
            self.pressed.remove(index);
            ops.push(WinInputOp::Key { key, up: true });
        } else {
            self.stats.stray_up += 1;
        }
        ops
    }

    fn plan_text(&mut self, text: &str) -> Vec<WinInputOp> {
        let plan = plan_text(text);
        self.stats.dropped += plan.dropped_controls as u64;

        let held_mods = self.mods;
        self.mods = Mods::NONE;
        let mut ops = Vec::with_capacity(held_mods.bits().count_ones() as usize + plan.steps.len());
        for &(modifier, usage) in MODIFIERS.iter().rev() {
            if held_mods.contains(modifier) {
                if let Some(key) = hid_to_win(usage) {
                    ops.push(WinInputOp::Key { key, up: true });
                }
            }
        }

        for step in plan.steps {
            match step {
                TextStep::Unicode(units) => {
                    for unit in units {
                        ops.push(WinInputOp::Unicode { unit, up: false });
                        ops.push(WinInputOp::Unicode { unit, up: true });
                    }
                }
                TextStep::Key(vk) => {
                    if let Some(key) = text_key(vk) {
                        ops.push(WinInputOp::Key { key, up: false });
                        ops.push(WinInputOp::Key { key, up: true });
                    }
                }
            }
        }

        for &(modifier, usage) in &MODIFIERS {
            if !held_mods.contains(modifier) {
                continue;
            }
            if let Some(key) = hid_to_win(usage) {
                ops.push(WinInputOp::Key { key, up: false });
            }
            self.mods |= modifier;
        }
        ops
    }

    fn plan_scroll(&mut self, msg: &InputMsg) -> Vec<WinInputOp> {
        let InputMsg::Scroll {
            dx,
            dy,
            phase,
            unit,
            ..
        } = *msg
        else {
            return Vec::new();
        };
        if !dx.is_finite() || !dy.is_finite() {
            self.stats.dropped += 1;
            return Vec::new();
        }
        if phase == ScrollPhase::Began {
            self.remainder_x = 0.0;
            self.remainder_y = 0.0;
        }

        let scale = match unit {
            ScrollUnit::Line => WHEEL_UNITS_PER_LINE,
            ScrollUnit::Pixel => self.wheel_units_per_pixel,
        };
        let vertical = accumulate(f64::from(dy) * scale, &mut self.remainder_y);
        let horizontal = accumulate(
            f64::from(dx) * scale * f64::from(HWHEEL_SIGN),
            &mut self.remainder_x,
        );

        let mut ops = Vec::with_capacity(2);
        if vertical != 0 {
            ops.push(WinInputOp::Wheel { delta: vertical });
        }
        if horizontal != 0 {
            ops.push(WinInputOp::HWheel { delta: horizontal });
        }
        ops
    }

    fn sync_modifiers(&mut self, target: Mods, ops: &mut Vec<WinInputOp>) {
        for &(modifier, usage) in &MODIFIERS {
            let wanted = target.contains(modifier);
            if wanted == self.mods.contains(modifier) {
                continue;
            }
            if let Some(key) = hid_to_win(usage) {
                ops.push(WinInputOp::Key { key, up: !wanted });
            }
            if wanted {
                self.mods |= modifier;
            } else {
                self.mods = Mods::from_bits(self.mods.bits() & !modifier.bits());
            }
        }
    }

    fn move_abs(&self, x: f32, y: f32, monitor: ScreenRect, desk: ScreenRect) -> WinInputOp {
        let (x, y) = to_virtual_desk(x, y, monitor, desk);
        WinInputOp::MoveAbs { x, y }
    }
}

/// Convert normalized monitor-relative coordinates to the virtual desktop.
pub fn to_virtual_desk(x: f32, y: f32, monitor: ScreenRect, desk: ScreenRect) -> (u16, u16) {
    (
        axis_to_virtual(x, monitor.left, monitor.width, desk.left, desk.width),
        axis_to_virtual(y, monitor.top, monitor.height, desk.top, desk.height),
    )
}

/// Convert an M0 input event to RSP input for [`WinPlanner::plan`].
pub fn input_from_m0(event: &InputEvent) -> InputMsg {
    match *event {
        InputEvent::MouseMove { x, y } => InputMsg::MouseMove { x, y },
        InputEvent::MouseButton {
            button,
            down,
            x,
            y,
            clicks,
        } => InputMsg::MouseButton {
            button: match button {
                rsp_core::input::MouseButton::Left => Button::Left,
                rsp_core::input::MouseButton::Right => Button::Right,
                rsp_core::input::MouseButton::Middle => Button::Middle,
            },
            down,
            x,
            y,
            clicks,
        },
        InputEvent::Scroll { dx, dy } => InputMsg::Scroll {
            dx,
            dy,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Pixel,
        },
    }
}

fn axis_to_virtual(
    position: f32,
    monitor_start: i32,
    monitor_size: u32,
    desk_start: i32,
    desk_size: u32,
) -> u16 {
    let normalized = if position.is_nan() {
        0.0
    } else {
        f64::from(position.clamp(0.0, 1.0))
    };
    let monitor_span = f64::from(monitor_size.saturating_sub(1));
    let pixel = f64::from(monitor_start) + (normalized * monitor_span).round();

    let desk_extent = if desk_size == 0 { 1 } else { desk_size };
    let desk_span = f64::from(desk_extent.saturating_sub(1).max(1));
    let virtual_position = ((pixel - f64::from(desk_start)) * 65535.0 / desk_span).round();
    virtual_position.clamp(0.0, 65535.0) as u16
}

fn button_parts(button: Button) -> (usize, WinButton) {
    match button {
        Button::Left => (0, WinButton::Left),
        Button::Right => (1, WinButton::Right),
        Button::Middle => (2, WinButton::Middle),
        Button::Back => (3, WinButton::X1),
        Button::Forward => (4, WinButton::X2),
    }
}

fn text_key(vk: u16) -> Option<WinKey> {
    let scan = match vk {
        VK_RETURN => 0x1C,
        VK_TAB => 0x0F,
        _ => return None,
    };
    Some(WinKey {
        scan,
        extended: false,
        vk: 0,
    })
}

fn accumulate(delta: f64, remainder: &mut f64) -> i32 {
    let sum = delta + *remainder;
    let whole = sum.clamp(f64::from(i32::MIN), f64::from(i32::MAX)).trunc() as i32;
    *remainder = if sum >= f64::from(i32::MAX) || sum <= f64::from(i32::MIN) {
        0.0
    } else {
        sum - f64::from(whole)
    };
    whole
}
