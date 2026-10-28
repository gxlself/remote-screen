//! RSP/1 输入 → Linux 注入操作（portal RemoteDesktop 与 X11 XTest 共用，纯逻辑）。

use host_core::inject::text_plan::{plan_text, TextStep, VK_RETURN, VK_TAB};
use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit};

use crate::keymap::{
    button_to_evdev, char_to_keysym, hid_to_evdev, modifier_bit, KEY_ENTER, KEY_TAB,
    MODIFIER_USAGES,
};

/// 滚动结束后多久没有惯性就发 AxisStop（03 §6.1 的 300 ms）。
pub const SCROLL_STOP_US: u64 = 300_000;
/// 一格滚轮的单位数（沿用 Windows WHEEL_DELTA）。
pub const UNITS_PER_STEP: f64 = 120.0;
/// 一行 = 40 单位（3 行一格）。
pub const UNITS_PER_LINE: f64 = 40.0;
/// 一像素 = 1.2 单位（与 win_plan 相同）。
pub const UNITS_PER_PIXEL: f64 = 1.2;
/// 按住的普通键上限。
pub const MAX_PRESSED: usize = 32;

const BUTTON_ORDER: [Button; 5] = [
    Button::Left,
    Button::Right,
    Button::Middle,
    Button::Back,
    Button::Forward,
];

/// 离散滚轮的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelAxis {
    Vertical,
    Horizontal,
}

/// 一次平台无关的 Linux 注入操作。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LinuxInputOp {
    /// evdev 键码（X11 注入时 +8）。
    Key { code: u16, down: bool },
    /// 文本字符的 keysym（portal NotifyKeyboardKeysym / X11 按 keysym 找键码）。
    Keysym { sym: u32, down: bool },
    /// 当前显示器内的绝对位置，单位是该显示器的逻辑像素（X11 再加显示器原点）。
    MoveAbs { x: f64, y: f64 },
    /// evdev 按钮码（BTN_LEFT 等）。
    Button { code: u16, down: bool },
    /// 平滑滚动，逻辑像素；正 = 视图向下 / 向右滚。只在 Portal 下出现。
    Axis { dx: f64, dy: f64 },
    /// 平滑滚动结束（portal NotifyPointerAxis(finish = true, 0, 0)）。
    AxisStop,
    /// 滚轮格数；正 = 视图向下 / 向右滚。
    Wheel { axis: WheelAxis, steps: i32 },
}

/// 根据后端选择平滑或离散滚动。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollMode {
    /// 像素单位 → Axis（平滑）；行单位 → Wheel（离散）。
    Portal,
    /// 像素、行都换算成 Wheel（X11 按钮 4/5/6/7）。
    X11,
}

/// 目标显示器的逻辑尺寸。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayGeom {
    pub width: f64,
    pub height: f64,
}

/// 自规划器创建以来拒绝或丢弃的输入计数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinuxInputStats {
    pub unknown_usage: u64,
    pub stray_up: u64,
    pub overflow: u64,
    pub dropped: u64,
}

/// 记录按住的键、按钮和滚动阶段的 RSP 输入规划器。
#[derive(Debug)]
pub struct LinuxPlanner {
    mode: ScrollMode,
    pressed: Vec<(u16, u16)>,
    mods: Mods,
    buttons: [bool; 5],
    remainder_x: f64,
    remainder_y: f64,
    scroll_deadline: Option<u64>,
    stats: LinuxInputStats,
}

impl LinuxPlanner {
    /// 使用指定后端的滚动模式创建规划器。
    pub fn new(mode: ScrollMode) -> Self {
        Self {
            mode,
            pressed: Vec::new(),
            mods: Mods::NONE,
            buttons: [false; 5],
            remainder_x: 0.0,
            remainder_y: 0.0,
            scroll_deadline: None,
            stats: LinuxInputStats::default(),
        }
    }

    /// 翻译一条输入；`now_us` 只用于滚动结束计时。
    pub fn plan(&mut self, msg: &InputMsg, display: DisplayGeom, now_us: u64) -> Vec<LinuxInputOp> {
        match msg {
            InputMsg::MouseMove { x, y } => self.move_abs(*x, *y, display).into_iter().collect(),
            InputMsg::MouseButton {
                button, down, x, y, ..
            } => self.plan_button(*button, *down, *x, *y, display),
            InputMsg::Key { usage, down, mods } => self.plan_key(*usage, *down, *mods),
            InputMsg::Text(text) => self.plan_text(text),
            InputMsg::Scroll { .. } => self.plan_scroll(msg, now_us),
            InputMsg::ReleaseAll => self.release_all(),
            InputMsg::Touch { .. } => {
                self.stats.dropped += 1;
                Vec::new()
            }
        }
    }

    /// 到期时返回 `[AxisStop]`，否则空。调用方在 `next_deadline()` 到时调用。
    pub fn poll(&mut self, now_us: u64) -> Vec<LinuxInputOp> {
        if self
            .scroll_deadline
            .is_some_and(|deadline| now_us >= deadline)
        {
            self.scroll_deadline = None;
            vec![LinuxInputOp::AxisStop]
        } else {
            Vec::new()
        }
    }

    /// 活动平滑滚动的收尾时间。
    pub fn next_deadline(&self) -> Option<u64> {
        self.scroll_deadline
    }

    /// 逆序松开普通键和修饰键，按协议顺序松开鼠标键，最后收尾活动平滑滚动。
    /// 清空滚动的小数累积，保留统计计数。
    pub fn release_all(&mut self) -> Vec<LinuxInputOp> {
        let mut ops =
            Vec::with_capacity(self.pressed.len() + MODIFIER_USAGES.len() + BUTTON_ORDER.len() + 1);
        for &(_, code) in self.pressed.iter().rev() {
            ops.push(LinuxInputOp::Key { code, down: false });
        }
        self.pressed.clear();
        self.release_modifiers(&mut ops);

        for (index, button) in BUTTON_ORDER.into_iter().enumerate() {
            if self.buttons[index] {
                self.buttons[index] = false;
                ops.push(LinuxInputOp::Button {
                    code: button_to_evdev(button),
                    down: false,
                });
            }
        }

        if self.scroll_deadline.take().is_some() {
            ops.push(LinuxInputOp::AxisStop);
        }
        self.remainder_x = 0.0;
        self.remainder_y = 0.0;
        ops
    }

    /// 自规划器创建以来累计的计数。
    pub fn stats(&self) -> LinuxInputStats {
        self.stats
    }

    fn move_abs(&mut self, x: f32, y: f32, display: DisplayGeom) -> Option<LinuxInputOp> {
        if !x.is_finite()
            || !y.is_finite()
            || !display.width.is_finite()
            || !display.height.is_finite()
        {
            self.stats.dropped += 1;
            return None;
        }
        Some(LinuxInputOp::MoveAbs {
            x: f64::from(x.clamp(0.0, 1.0)) * (display.width - 1.0).max(0.0),
            y: f64::from(y.clamp(0.0, 1.0)) * (display.height - 1.0).max(0.0),
        })
    }

    fn plan_button(
        &mut self,
        button: Button,
        down: bool,
        x: f32,
        y: f32,
        display: DisplayGeom,
    ) -> Vec<LinuxInputOp> {
        let Some(movement) = self.move_abs(x, y, display) else {
            return Vec::new();
        };
        let mut ops = vec![movement];
        let index = button as usize;
        if down || self.buttons[index] {
            self.buttons[index] = down;
            ops.push(LinuxInputOp::Button {
                code: button_to_evdev(button),
                down,
            });
        } else {
            self.stats.stray_up += 1;
        }
        ops
    }

    fn plan_key(&mut self, usage: u16, down: bool, mods: Mods) -> Vec<LinuxInputOp> {
        if let Some(own_bit) = modifier_bit(usage) {
            let target = if down {
                mods | own_bit
            } else {
                Mods::from_bits(mods.bits() & !own_bit.bits())
            };
            let mut ops = Vec::with_capacity(MODIFIER_USAGES.len());
            self.sync_modifiers(target, &mut ops);
            return ops;
        }

        let Some(code) = hid_to_evdev(usage) else {
            self.stats.unknown_usage += 1;
            return Vec::new();
        };
        let mut ops = Vec::with_capacity(MODIFIER_USAGES.len() + 1);
        self.sync_modifiers(mods, &mut ops);
        let index = self
            .pressed
            .iter()
            .position(|&(pressed_usage, _)| pressed_usage == usage);
        if down {
            if index.is_some() {
                ops.push(LinuxInputOp::Key { code, down });
            } else if self.pressed.len() >= MAX_PRESSED {
                self.stats.overflow += 1;
            } else {
                self.pressed.push((usage, code));
                ops.push(LinuxInputOp::Key { code, down });
            }
        } else if let Some(index) = index {
            self.pressed.remove(index);
            ops.push(LinuxInputOp::Key { code, down });
        } else {
            self.stats.stray_up += 1;
        }
        ops
    }

    fn sync_modifiers(&mut self, target: Mods, ops: &mut Vec<LinuxInputOp>) {
        for (modifier, usage) in MODIFIER_USAGES {
            let down = target.contains(modifier);
            if down != self.mods.contains(modifier) {
                if let Some(code) = hid_to_evdev(usage) {
                    ops.push(LinuxInputOp::Key { code, down });
                }
            }
        }
        self.mods = target;
    }

    fn release_modifiers(&mut self, ops: &mut Vec<LinuxInputOp>) {
        for &(modifier, usage) in MODIFIER_USAGES.iter().rev() {
            if self.mods.contains(modifier) {
                if let Some(code) = hid_to_evdev(usage) {
                    ops.push(LinuxInputOp::Key { code, down: false });
                }
            }
        }
        self.mods = Mods::NONE;
    }

    fn plan_text(&mut self, text: &str) -> Vec<LinuxInputOp> {
        let plan = plan_text(text);
        self.stats.dropped += plan.dropped_controls as u64;
        let held_mods = self.mods;
        let mut ops = Vec::new();
        self.release_modifiers(&mut ops);

        for step in plan.steps {
            match step {
                TextStep::Unicode(units) => {
                    for character in char::decode_utf16(units) {
                        if let Some(sym) = character.ok().and_then(char_to_keysym) {
                            ops.push(LinuxInputOp::Keysym { sym, down: true });
                            ops.push(LinuxInputOp::Keysym { sym, down: false });
                        } else {
                            self.stats.dropped += 1;
                        }
                    }
                }
                TextStep::Key(vk) => {
                    let code = match vk {
                        VK_RETURN => KEY_ENTER,
                        VK_TAB => KEY_TAB,
                        _ => continue,
                    };
                    ops.push(LinuxInputOp::Key { code, down: true });
                    ops.push(LinuxInputOp::Key { code, down: false });
                }
            }
        }

        self.sync_modifiers(held_mods, &mut ops);
        ops
    }

    fn plan_scroll(&mut self, msg: &InputMsg, now_us: u64) -> Vec<LinuxInputOp> {
        let InputMsg::Scroll {
            dx,
            dy,
            phase,
            momentum,
            unit,
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

        let dx = -f64::from(dx);
        let dy = -f64::from(dy);
        let mut ops = Vec::with_capacity(2);
        if self.mode == ScrollMode::Portal && unit == ScrollUnit::Pixel {
            if dx != 0.0 || dy != 0.0 {
                ops.push(LinuxInputOp::Axis { dx, dy });
            }
            if momentum == ScrollPhase::Ended {
                ops.push(LinuxInputOp::AxisStop);
                self.scroll_deadline = None;
            } else if matches!(phase, ScrollPhase::Began | ScrollPhase::Changed)
                || matches!(momentum, ScrollPhase::Began | ScrollPhase::Changed)
                || (phase == ScrollPhase::Ended && momentum == ScrollPhase::None)
            {
                self.scroll_deadline = Some(now_us.saturating_add(SCROLL_STOP_US));
            }
        } else {
            let scale = match unit {
                ScrollUnit::Pixel => UNITS_PER_PIXEL,
                ScrollUnit::Line => UNITS_PER_LINE,
            };
            let vertical = accumulate(dy * scale, &mut self.remainder_y);
            let horizontal = accumulate(dx * scale, &mut self.remainder_x);
            if vertical != 0 {
                ops.push(LinuxInputOp::Wheel {
                    axis: WheelAxis::Vertical,
                    steps: vertical,
                });
            }
            if horizontal != 0 {
                ops.push(LinuxInputOp::Wheel {
                    axis: WheelAxis::Horizontal,
                    steps: horizontal,
                });
            }
        }
        ops
    }
}

fn accumulate(delta: f64, remainder: &mut f64) -> i32 {
    let sum = *remainder + delta;
    let steps = (sum / UNITS_PER_STEP).trunc();
    // Saturate unrepresentable step counts and discard their excess, as win_plan does.
    *remainder = if steps >= f64::from(i32::MAX) || steps <= f64::from(i32::MIN) {
        0.0
    } else {
        sum - steps * UNITS_PER_STEP
    };
    steps as i32
}
