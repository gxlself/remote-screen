//! Key event sequencing for the macOS injector (task ).
//!
//! Converts protocol key events into the event sequence macOS needs to see:
//! normal key down/up events plus the `FlagsChanged` events that make modifier
//! state visible to system shortcuts. This module is pure and testable on Linux;
//! posting the events is the platform layer's responsibility.

use rsp_core::hid::Mods;

use crate::inject::keymap_mac::{modifier_usage_to_vk, mods_to_cg_flags};

/// `kCGEventFlagMaskAlphaShift`.
pub const FLAG_ALPHA_SHIFT: u64 = 0x0001_0000;
/// `kCGEventFlagMaskNumericPad`.
pub const FLAG_NUMERIC_PAD: u64 = 0x0020_0000;
/// `kCGEventFlagMaskSecondaryFn`.
pub const FLAG_SECONDARY_FN: u64 = 0x0080_0000;

/// Maximum number of ordinary keys tracked as held.
///
/// The protocol does not specify a limit. This bound keeps a misbehaving client
/// from growing the tracker without limit; further downs are counted instead.
pub const MAX_PRESSED: usize = 32;

/// `kVK_CapsLock`.
pub const VK_CAPS_LOCK: u16 = 0x39;

/// Modifier bits in the order used to emit modifier transitions.
pub const MOD_ORDER: [Mods; 8] = [
    Mods::LCTRL,
    Mods::LSHIFT,
    Mods::LALT,
    Mods::LMETA,
    Mods::RCTRL,
    Mods::RSHIFT,
    Mods::RALT,
    Mods::RMETA,
];

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

/// Flags that real Apple keyboards attach to particular virtual keys.
///
/// Mission Control's default shortcut is Control-Up. macOS stores the
/// SecondaryFn bit in the shortcut's modifier mask (`0x840000`), so the
/// injected Up arrow must carry `FLAG_SECONDARY_FN` as well. Hardware tests should
/// verify the exact behavior on real hardware.
pub fn implicit_flags(vk: u16) -> u64 {
    match vk {
        // kVK_LeftArrow ..= kVK_RightArrow
        0x7B..=0x7E => FLAG_NUMERIC_PAD | FLAG_SECONDARY_FN,

        // kVK_F1 ..= kVK_F20, Help, Home, PageUp, ForwardDelete, End, PageDown
        0x7A | 0x78 | 0x63 | 0x76 | 0x60 | 0x61 | 0x62 | 0x64 | 0x65 | 0x6D | 0x67 | 0x6F
        | 0x69 | 0x6B | 0x71 | 0x6A | 0x40 | 0x4F | 0x50 | 0x5A | 0x72 | 0x73 | 0x74 | 0x75
        | 0x77 | 0x79 => FLAG_SECONDARY_FN,

        // Keypad decimal, multiply, plus, clear, divide, enter, minus, equals,
        // digits, and the remaining keypad keys observed on Apple keyboards.
        0x41
        | 0x43
        | 0x45
        | 0x47
        | 0x4B
        | 0x4C
        | 0x4E
        | 0x51
        | 0x52..=0x59
        | 0x5B
        | 0x5C
        | 0x5F => FLAG_NUMERIC_PAD,

        _ => 0,
    }
}

/// Kind of event to post with CoreGraphics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacKeyKind {
    Down,
    Up,
    FlagsChanged,
}

/// One event in the sequence produced for a protocol key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacKeyEvent {
    pub kind: MacKeyKind,
    pub vk: u16,
    pub flags: u64,
    pub autorepeat: bool,
}

/// Counters for malformed or bounded-out input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyStats {
    pub unknown_usage: u64,
    pub stray_up: u64,
    pub overflow: u64,
}

/// Stateful key sequencer.
#[derive(Debug, Default)]
pub struct KeyTracker {
    pressed: Vec<(u16, u16)>,
    mods: Mods,
    caps: bool,
    stats: KeyStats,
}

impl KeyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Convert one protocol key event into zero or more macOS events.
    pub fn on_key(
        &mut self,
        usage: u16,
        down: bool,
        mods: Mods,
        resolve: &dyn Fn(u16) -> Option<u16>,
    ) -> Vec<MacKeyEvent> {
        if let Some(own_bit) = modifier_for_usage(usage) {
            // A modifier's own bit is authoritative even when the sender's
            // aggregate `mods` value disagrees with `down`.
            let target = if down {
                mods | own_bit
            } else {
                Mods::from_bits(mods.bits() & !own_bit.bits())
            };
            return self.sync(target, Some(usage));
        }

        if usage == VK_CAPS_LOCK {
            let mut events = self.sync(mods, None);
            if down {
                self.caps = !self.caps;
                // An injected FlagsChanged event may not actually toggle the
                // system Caps Lock state. Hardware tests must verify this on hardware
                // and may need IOHIDSetModifierLockState; this tracker only
                // maintains the flags it reports to the posting layer.
                events.push(MacKeyEvent {
                    kind: MacKeyKind::FlagsChanged,
                    vk: VK_CAPS_LOCK,
                    flags: self.current_flags(),
                    autorepeat: false,
                });
            }
            return events;
        }

        let Some(vk) = resolve(usage) else {
            self.stats.unknown_usage += 1;
            return Vec::new();
        };

        let mut events = self.sync(mods, None);
        if down {
            if self
                .pressed
                .iter()
                .any(|&(pressed_usage, _)| pressed_usage == usage)
            {
                events.push(self.key_event(MacKeyKind::Down, vk, true));
            } else if self.pressed.len() >= MAX_PRESSED {
                self.stats.overflow += 1;
            } else {
                self.pressed.push((usage, vk));
                events.push(self.key_event(MacKeyKind::Down, vk, false));
            }
        } else if let Some(index) = self
            .pressed
            .iter()
            .position(|&(pressed_usage, _)| pressed_usage == usage)
        {
            self.pressed.remove(index);
            events.push(self.key_event(MacKeyKind::Up, vk, false));
        } else {
            self.stats.stray_up += 1;
        }
        events
    }

    /// Release every held key and modifier, leaving Caps Lock unchanged.
    pub fn release_all(&mut self) -> Vec<MacKeyEvent> {
        let mut events = Vec::with_capacity(self.pressed.len() + MODIFIERS.len());
        let pressed = std::mem::take(&mut self.pressed);
        for &(_, vk) in pressed.iter().rev() {
            events.push(self.key_event(MacKeyKind::Up, vk, false));
        }

        for &(modifier, usage) in MODIFIERS.iter().rev() {
            if !self.mods.contains(modifier) {
                continue;
            }
            self.mods = Mods::from_bits(self.mods.bits() & !modifier.bits());
            if let Some(vk) = modifier_usage_to_vk(usage) {
                events.push(MacKeyEvent {
                    kind: MacKeyKind::FlagsChanged,
                    vk,
                    flags: self.current_flags(),
                    autorepeat: false,
                });
            }
        }
        events
    }

    pub fn mods(&self) -> Mods {
        self.mods
    }

    pub fn caps_lock(&self) -> bool {
        self.caps
    }

    pub fn pressed(&self) -> usize {
        self.pressed.len()
    }

    pub fn stats(&self) -> KeyStats {
        self.stats
    }

    pub fn current_flags(&self) -> u64 {
        mods_to_cg_flags(self.mods) | if self.caps { FLAG_ALPHA_SHIFT } else { 0 }
    }

    fn sync(&mut self, target: Mods, last: Option<u16>) -> Vec<MacKeyEvent> {
        let mut events = Vec::with_capacity(MODIFIERS.len());
        for &(modifier, usage) in &MODIFIERS {
            if Some(usage) != last {
                self.sync_modifier(modifier, usage, target, &mut events);
            }
        }
        if let Some(last_usage) = last {
            if let Some(&(modifier, usage)) = MODIFIERS
                .iter()
                .find(|&&(_, candidate)| candidate == last_usage)
            {
                self.sync_modifier(modifier, usage, target, &mut events);
            }
        }
        events
    }

    fn sync_modifier(
        &mut self,
        modifier: Mods,
        usage: u16,
        target: Mods,
        events: &mut Vec<MacKeyEvent>,
    ) {
        let wanted = target.contains(modifier);
        if wanted == self.mods.contains(modifier) {
            return;
        }

        if wanted {
            self.mods |= modifier;
        } else {
            self.mods = Mods::from_bits(self.mods.bits() & !modifier.bits());
        }
        if let Some(vk) = modifier_usage_to_vk(usage) {
            events.push(MacKeyEvent {
                kind: MacKeyKind::FlagsChanged,
                vk,
                flags: self.current_flags(),
                autorepeat: false,
            });
        }
    }

    fn key_event(&self, kind: MacKeyKind, vk: u16, autorepeat: bool) -> MacKeyEvent {
        MacKeyEvent {
            kind,
            vk,
            flags: self.current_flags() | implicit_flags(vk),
            autorepeat,
        }
    }
}

fn modifier_for_usage(usage: u16) -> Option<Mods> {
    MODIFIERS
        .iter()
        .find_map(|&(modifier, candidate)| (candidate == usage).then_some(modifier))
}
