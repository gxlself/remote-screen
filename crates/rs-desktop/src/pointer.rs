//! 电脑端指针映射（03 §6.1）。

use rsp_core::hid::{Button, InputMsg};

use crate::platform::{PointerCapture, PointerInput};

/// 视图（逻辑点）与视频（像素）尺寸。视频等比缩放、居中、加黑边。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub view_w: f64,
    pub view_h: f64,
    pub video_w: u32,
    pub video_h: u32,
}

impl Viewport {
    /// 视频内容在视图里的矩形 `(x, y, w, h)`。
    ///
    /// 未知视频尺寸时，内容占满视图。视图尺寸来自窗口布局，正常情况
    /// 下为正数；对异常的非有限值也采用占满视图的退化矩形，避免把
    /// 非有限坐标送入协议。
    pub fn content_rect(&self) -> (f64, f64, f64, f64) {
        let view_w = if self.view_w.is_finite() {
            self.view_w.max(0.0)
        } else {
            0.0
        };
        let view_h = if self.view_h.is_finite() {
            self.view_h.max(0.0)
        } else {
            0.0
        };

        if self.video_w == 0 || self.video_h == 0 || view_w == 0.0 || view_h == 0.0 {
            return (0.0, 0.0, view_w, view_h);
        }

        let video_w = f64::from(self.video_w);
        let video_h = f64::from(self.video_h);
        let scale = (view_w / video_w).min(view_h / video_h);
        let content_w = video_w * scale;
        let content_h = video_h * scale;
        (
            (view_w - content_w) / 2.0,
            (view_h - content_h) / 2.0,
            content_w,
            content_h,
        )
    }

    /// 内容区外（含黑边）返回 `None`。
    pub fn to_normalized(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let (left, top, width, height) = self.content_rect();
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let right = left + width;
        let bottom = top + height;
        if x < left || x > right || y < top || y > bottom {
            return None;
        }
        let nx = ((x - left) / width) as f32;
        let ny = ((y - top) / height) as f32;
        (nx.is_finite() && ny.is_finite()).then_some((nx, ny))
    }

    /// 夹到内容区边上，用于拖动出界和抬起。
    pub fn to_normalized_clamped(&self, x: f64, y: f64) -> (f32, f32) {
        let (left, top, width, height) = self.content_rect();
        if width <= 0.0 || height <= 0.0 {
            return (0.0, 0.0);
        }
        let x = if x.is_finite() { x } else { left };
        let y = if y.is_finite() { y } else { top };
        let nx = ((x - left) / width).clamp(0.0, 1.0) as f32;
        let ny = ((y - top) / height).clamp(0.0, 1.0) as f32;
        (nx, ny)
    }

    /// 归一化坐标转换回视图坐标。
    pub fn to_view(&self, nx: f32, ny: f32) -> (f64, f64) {
        let (left, top, width, height) = self.content_rect();
        (left + f64::from(nx) * width, top + f64::from(ny) * height)
    }
}

/// 最新值优先的节流器。
#[derive(Debug, Clone)]
pub struct PointerThrottle {
    interval_us: u64,
    last_sent_us: Option<u64>,
    pending: Option<(f32, f32)>,
}

impl PointerThrottle {
    pub fn new(interval_us: u64) -> Self {
        Self {
            interval_us,
            last_sent_us: None,
            pending: None,
        }
    }

    /// 距上次发送达到间隔时立即发送，否则只保留最新值。
    pub fn push(&mut self, x: f32, y: f32, now_us: u64) -> Option<(f32, f32)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let due = self
            .last_sent_us
            .is_none_or(|last| now_us.saturating_sub(last) >= self.interval_us);
        if due {
            self.last_sent_us = Some(now_us);
            self.pending = None;
            Some((x, y))
        } else {
            self.pending = Some((x, y));
            None
        }
    }

    /// 有 pending 且已到间隔时返回它。
    pub fn flush(&mut self, now_us: u64) -> Option<(f32, f32)> {
        let value = self.pending?;
        let due = self
            .last_sent_us
            .is_none_or(|last| now_us.saturating_sub(last) >= self.interval_us);
        if due {
            self.pending = None;
            self.last_sent_us = Some(now_us);
            Some(value)
        } else {
            None
        }
    }

    /// 不管间隔，取出 pending。
    pub fn take_pending(&mut self) -> Option<(f32, f32)> {
        self.pending.take()
    }

    /// 清空节流器。下一次位置事件可立即发送。
    pub fn clear(&mut self) {
        self.pending = None;
        self.last_sent_us = None;
    }
}

/// 连击计数器。
#[derive(Debug, Clone)]
pub struct ClickCounter {
    interval_us: u64,
    slop: f64,
    last: Option<ClickRecord>,
}

#[derive(Debug, Clone, Copy)]
struct ClickRecord {
    button: u8,
    x: f64,
    y: f64,
    at_us: u64,
    clicks: u8,
}

impl ClickCounter {
    pub fn new(interval_us: u64, slop: f64) -> Self {
        Self {
            interval_us,
            slop: slop.max(0.0),
            last: None,
        }
    }

    /// 只在按下时调用，返回本次的连击数。
    pub fn down(&mut self, button: u8, x: f64, y: f64, now_us: u64) -> u8 {
        let clicks = match self.last {
            Some(last)
                if last.button == button
                    && now_us >= last.at_us
                    && now_us - last.at_us <= self.interval_us
                    && (x - last.x).hypot(y - last.y) <= self.slop =>
            {
                if last.clicks >= 3 {
                    1
                } else {
                    last.clicks + 1
                }
            }
            _ => 1,
        };
        self.last = Some(ClickRecord {
            button,
            x,
            y,
            at_us: now_us,
            clicks,
        });
        clicks
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PointerOut {
    /// 走 `SessionHandle::send_pointer`。
    Pointer { x: f32, y: f32 },
    /// 走 `SessionHandle::send_input`。
    Input(InputMsg),
    /// 转成 `WindowCommand::MoveVirtualCursor`。
    VirtualCursor { x: f32, y: f32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerConfig {
    /// 默认 8_333（120 Hz）。
    pub throttle_us: u64,
    /// 默认 500_000。
    pub double_click_us: u64,
    /// 默认 4.0。
    pub click_slop: f64,
}

impl Default for PointerConfig {
    fn default() -> Self {
        Self {
            throttle_us: 8_333,
            double_click_us: 500_000,
            click_slop: 4.0,
        }
    }
}

pub struct PointerState {
    viewport: Viewport,
    capture: PointerCapture,
    throttle: PointerThrottle,
    clicks: ClickCounter,
    held: u8,
    held_clicks: [u8; 5],
    host_position: Option<(f32, f32)>,
    last_position: Option<(f32, f32)>,
    virtual_position: Option<(f32, f32)>,
}

impl PointerState {
    pub fn new(cfg: PointerConfig) -> Self {
        Self {
            viewport: Viewport {
                view_w: 0.0,
                view_h: 0.0,
                video_w: 0,
                video_h: 0,
            },
            capture: PointerCapture::Free,
            throttle: PointerThrottle::new(cfg.throttle_us),
            clicks: ClickCounter::new(cfg.double_click_us, cfg.click_slop),
            held: 0,
            held_clicks: [0; 5],
            host_position: None,
            last_position: None,
            virtual_position: None,
        }
    }

    pub fn set_viewport(&mut self, vp: Viewport) {
        self.viewport = vp;
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// 进入 Virtual 时选取最近的已知位置并返回要画的虚拟光标。
    pub fn set_capture(&mut self, mode: PointerCapture) -> Vec<PointerOut> {
        let entering_virtual =
            self.capture != PointerCapture::Virtual && mode == PointerCapture::Virtual;
        self.capture = mode;
        if !entering_virtual {
            return Vec::new();
        }

        let position = self
            .last_position
            .or(self.host_position)
            .unwrap_or((0.5, 0.5));
        let position = (clamp_unit(position.0), clamp_unit(position.1));
        self.virtual_position = Some(position);
        self.last_position = Some(position);
        vec![PointerOut::VirtualCursor {
            x: position.0,
            y: position.1,
        }]
    }

    pub fn capture(&self) -> PointerCapture {
        self.capture
    }

    pub fn on_input(&mut self, ev: PointerInput, now_us: u64) -> Vec<PointerOut> {
        match ev {
            PointerInput::Move { x, y, dx, dy } => self.on_move(x, y, dx, dy, now_us),
            PointerInput::Button {
                button,
                down,
                x,
                y,
                clicks,
            } => self.on_button(button, down, x, y, clicks, now_us),
            PointerInput::Leave => {
                self.throttle.clear();
                Vec::new()
            }
        }
    }

    /// 只记录 Host 光标位置，不产生输出。
    pub fn on_host_pointer(&mut self, x: f32, y: f32) {
        if x.is_finite() && y.is_finite() {
            let position = (clamp_unit(x), clamp_unit(y));
            self.host_position = Some(position);
            self.last_position = Some(position);
        }
    }

    pub fn flush(&mut self, now_us: u64) -> Vec<PointerOut> {
        let Some((x, y)) = self.throttle.flush(now_us) else {
            return Vec::new();
        };
        self.last_position = Some((x, y));
        self.position_output(x, y)
    }

    pub fn held_buttons(&self) -> u8 {
        self.held
    }

    pub fn release_all(&mut self) -> bool {
        let had_held = self.held != 0;
        self.held = 0;
        self.held_clicks = [0; 5];
        self.throttle.clear();
        had_held
    }

    fn on_move(&mut self, x: f64, y: f64, dx: f64, dy: f64, now_us: u64) -> Vec<PointerOut> {
        if self.capture == PointerCapture::Virtual {
            if !dx.is_finite() || !dy.is_finite() {
                return Vec::new();
            }
            let (mut nx, mut ny) = self.virtual_position.unwrap_or((0.5, 0.5));
            let (_, _, content_w, content_h) = self.viewport.content_rect();
            if content_w > 0.0 {
                nx = (f64::from(nx) + dx / content_w).clamp(0.0, 1.0) as f32;
            }
            if content_h > 0.0 {
                ny = (f64::from(ny) + dy / content_h).clamp(0.0, 1.0) as f32;
            }
            let next = (clamp_unit(nx), clamp_unit(ny));
            let changed = self.virtual_position.is_none_or(|old| old != next);
            self.virtual_position = Some(next);
            self.last_position = Some(next);
            if !changed {
                return Vec::new();
            }
            return self.push_position(next.0, next.1, now_us);
        }

        if !x.is_finite() || !y.is_finite() {
            return Vec::new();
        }
        let normalized = self.viewport.to_normalized(x, y);
        let normalized = match normalized {
            Some(position) => Some(position),
            None if self.held != 0 => Some(self.viewport.to_normalized_clamped(x, y)),
            None => None,
        };
        let Some((nx, ny)) = normalized else {
            return Vec::new();
        };
        self.last_position = Some((nx, ny));
        self.push_position(nx, ny, now_us)
    }

    fn on_button(
        &mut self,
        button: u8,
        down: bool,
        x: f64,
        y: f64,
        platform_clicks: u8,
        now_us: u64,
    ) -> Vec<PointerOut> {
        if button > 4 {
            return Vec::new();
        }
        let bit = 1u8 << button;

        if down {
            let (nx, ny, click_x, click_y) = if self.capture == PointerCapture::Virtual {
                let (nx, ny) = self.virtual_position.unwrap_or((0.5, 0.5));
                let (click_x, click_y) = self.viewport.to_view(nx, ny);
                (nx, ny, click_x, click_y)
            } else {
                let Some(position) = self.viewport.to_normalized(x, y) else {
                    return Vec::new();
                };
                (position.0, position.1, x, y)
            };
            self.last_position = Some((nx, ny));
            let clicks = if platform_clicks > 0 {
                platform_clicks
            } else {
                self.clicks.down(button, click_x, click_y, now_us)
            };
            // The button carries its own coordinate, so pending motion must be
            // discarded before the button message is sent.
            self.throttle.take_pending();
            self.held |= bit;
            self.held_clicks[usize::from(button)] = clicks;
            vec![PointerOut::Input(InputMsg::MouseButton {
                button: button_from_u8(button),
                down: true,
                x: nx,
                y: ny,
                clicks,
            })]
        } else {
            if self.held & bit == 0 {
                return Vec::new();
            }
            if self.capture != PointerCapture::Virtual && (!x.is_finite() || !y.is_finite()) {
                return Vec::new();
            }
            let (nx, ny) = if self.capture == PointerCapture::Virtual {
                self.virtual_position.unwrap_or((0.5, 0.5))
            } else {
                self.viewport.to_normalized_clamped(x, y)
            };
            let clicks = self.held_clicks[usize::from(button)].max(1);
            self.held &= !bit;
            self.held_clicks[usize::from(button)] = 0;
            vec![PointerOut::Input(InputMsg::MouseButton {
                button: button_from_u8(button),
                down: false,
                x: nx,
                y: ny,
                clicks,
            })]
        }
    }

    fn push_position(&mut self, x: f32, y: f32, now_us: u64) -> Vec<PointerOut> {
        let Some((x, y)) = self.throttle.push(x, y, now_us) else {
            return Vec::new();
        };
        self.position_output(x, y)
    }

    fn position_output(&self, x: f32, y: f32) -> Vec<PointerOut> {
        if self.capture == PointerCapture::Virtual {
            vec![
                PointerOut::Pointer { x, y },
                PointerOut::VirtualCursor { x, y },
            ]
        } else {
            vec![PointerOut::Pointer { x, y }]
        }
    }
}

fn button_from_u8(button: u8) -> Button {
    match button {
        0 => Button::Left,
        1 => Button::Right,
        2 => Button::Middle,
        3 => Button::Back,
        4 => Button::Forward,
        _ => unreachable!("button validated before conversion"),
    }
}

fn clamp_unit(value: f32) -> f32 {
    if value.is_nan() {
        0.5
    } else {
        value.clamp(0.0, 1.0)
    }
}
