//! 电脑端滚动映射（03 §6.1 Scroll）。

use rsp_core::hid::{InputMsg, ScrollPhase, ScrollUnit};

use crate::platform::ScrollInput;

/// NSEvent.Phase 原始位 → 协议阶段。
///
/// 外层 Option 的 None 表示仅有 mayBegin，该事件必须丢弃；
/// Some(ScrollPhase::None) 表示普通的无相位滚动。
pub fn mac_phase(bits: u64) -> Option<ScrollPhase> {
    const BEGAN: u64 = 1;
    const STATIONARY: u64 = 2;
    const CHANGED: u64 = 4;
    const ENDED: u64 = 8;
    const CANCELLED: u64 = 16;
    const MAY_BEGIN: u64 = 32;

    if bits & MAY_BEGIN != 0 && bits & (BEGAN | STATIONARY | CHANGED | ENDED | CANCELLED) == 0 {
        return None;
    }
    if bits & BEGAN != 0 {
        Some(ScrollPhase::Began)
    } else if bits & (CHANGED | STATIONARY) != 0 {
        Some(ScrollPhase::Changed)
    } else if bits & (ENDED | CANCELLED) != 0 {
        Some(ScrollPhase::Ended)
    } else {
        Some(ScrollPhase::None)
    }
}

/// Windows 1 行对应的滚轮单位数。
pub const WIN_WHEEL_UNITS_PER_LINE: f64 = 40.0;

#[derive(Debug, Default, Clone)]
pub struct ScrollMapper;

impl ScrollMapper {
    pub fn new() -> Self {
        Self
    }

    /// 返回 None 表示丢弃。
    pub fn map(&mut self, ev: ScrollInput) -> Option<InputMsg> {
        match ev {
            ScrollInput::MacPrecise {
                dx,
                dy,
                phase,
                momentum,
            } => self.map_mac_precise(dx, dy, phase, momentum),
            ScrollInput::MacLines { dx, dy } => {
                let (dx, dy) = finite_pair(dx, dy)?;
                if dx == 0.0 && dy == 0.0 {
                    return None;
                }
                Some(InputMsg::Scroll {
                    dx: finite_f32(dx)?,
                    dy: finite_f32(dy)?,
                    phase: ScrollPhase::None,
                    momentum: ScrollPhase::None,
                    unit: ScrollUnit::Line,
                })
            }
            ScrollInput::WinWheel { delta, horizontal } => {
                if delta == 0 {
                    return None;
                }
                let amount = f64::from(delta) / WIN_WHEEL_UNITS_PER_LINE;
                let (dx, dy) = if horizontal {
                    (-amount, 0.0)
                } else {
                    (0.0, amount)
                };
                Some(InputMsg::Scroll {
                    dx: finite_f32(dx)?,
                    dy: finite_f32(dy)?,
                    phase: ScrollPhase::None,
                    momentum: ScrollPhase::None,
                    unit: ScrollUnit::Line,
                })
            }
        }
    }

    fn map_mac_precise(
        &mut self,
        dx: f64,
        dy: f64,
        phase_bits: u64,
        momentum_bits: u64,
    ) -> Option<InputMsg> {
        let (dx, dy) = finite_pair(dx, dy)?;
        let phase = mac_phase(phase_bits)?;
        let momentum = mac_phase(momentum_bits)?;

        // Gesture and momentum at the same time is ambiguous to the Host
        // injector, so discard the complete event.
        if phase != ScrollPhase::None && momentum != ScrollPhase::None {
            return None;
        }
        if phase == ScrollPhase::None && momentum == ScrollPhase::None && dx == 0.0 && dy == 0.0 {
            return None;
        }
        // AppKit reports stationary with no movement for bookkeeping events.
        if phase_bits & 2 != 0 && dx == 0.0 && dy == 0.0 {
            return None;
        }

        Some(InputMsg::Scroll {
            dx: finite_f32(dx)?,
            dy: finite_f32(dy)?,
            phase,
            momentum,
            unit: ScrollUnit::Pixel,
        })
    }
}

fn finite_pair(dx: f64, dy: f64) -> Option<(f64, f64)> {
    (dx.is_finite() && dy.is_finite()).then_some((dx, dy))
}

fn finite_f32(value: f64) -> Option<f32> {
    let value = value as f32;
    value.is_finite().then_some(value)
}
