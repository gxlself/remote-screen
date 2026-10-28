//! 电脑端键盘路由（03 §6.1；07 · 电脑端）。

use rsp_core::hid::{InputMsg, Mods};

use crate::{
    keymap::{mac_flags_change, mac_vk_to_hid, win_scan_to_hid, MacFlagsChange, MacKeyboardKind},
    platform::KeyInput,
};

// ClientOs / HostOs are part of the platform contract, but are re-exported
// here because keyboard callers should not need to import that module too.
pub use crate::platform::{ClientOs, HostOs};

const FIRST_MODIFIER: u16 = 0xE0;
const LAST_MODIFIER: u16 = 0xE7;
const LEFT_CTRL: u16 = 0xE0;
const LEFT_ALT: u16 = 0xE2;
const LEFT_META: u16 = 0xE3;
const RIGHT_META: u16 = 0xE7;
const CAPS_LOCK: u16 = 0x39;
const MAX_PRESSED: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierRemap {
    None,
    /// ⌘ → Ctrl：0xE3→0xE0、0xE7→0xE4（Mac 客户端连 Windows Host 的默认）。
    CmdToCtrl,
    /// Ctrl ↔ ⌘ 互换：0xE0↔0xE3、0xE4↔0xE7（Windows 客户端连 Mac Host 的可选项）。
    SwapCtrlMeta,
}

/// (Mac, Windows) → CmdToCtrl；其他组合 → None（含 Host 未知）。
pub fn default_remap(client: ClientOs, host: HostOs) -> ModifierRemap {
    match (client, host) {
        (ClientOs::Mac, HostOs::Windows) => ModifierRemap::CmdToCtrl,
        _ => ModifierRemap::None,
    }
}

/// 只改上面列出的修饰键用法码，其他原样返回。
pub fn remap_usage(usage: u16, remap: ModifierRemap) -> u16 {
    match remap {
        ModifierRemap::None => usage,
        ModifierRemap::CmdToCtrl => match usage {
            LEFT_META => LEFT_CTRL,
            RIGHT_META => 0xE4,
            _ => usage,
        },
        ModifierRemap::SwapCtrlMeta => match usage {
            LEFT_CTRL => LEFT_META,
            0xE4 => RIGHT_META,
            LEFT_META => LEFT_CTRL,
            RIGHT_META => 0xE4,
            _ => usage,
        },
    }
}

/// 左右区分的修饰键状态。
#[derive(Debug, Clone, Copy, Default)]
pub struct ModTracker {
    bits: u8,
}

impl ModTracker {
    /// 0xE0..=0xE7 置位 / 清位；返回更新后的状态。其他 usage 不改状态，返回当前状态。
    pub fn apply(&mut self, usage: u16, down: bool) -> Mods {
        if (FIRST_MODIFIER..=LAST_MODIFIER).contains(&usage) {
            let bit = 1u8 << (usage - FIRST_MODIFIER);
            if down {
                self.bits |= bit;
            } else {
                self.bits &= !bit;
            }
        }
        self.mods()
    }

    pub fn mods(&self) -> Mods {
        Mods::from_bits(self.bits)
    }

    pub fn clear(&mut self) {
        self.bits = 0;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeyAction {
    /// 发给远端。
    Send(InputMsg),
    /// 和弦触发：释放键盘捕获。之后按键归本机。
    ReleaseCapture,
    /// 释放状态下和弦再触发：恢复捕获。
    ResumeCapture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyRouterConfig {
    pub client: ClientOs,
    pub host: HostOs,
    pub remap: ModifierRemap,
    pub mac_kind: MacKeyboardKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChordState {
    Idle,
    Armed(u8),
    Spoiled,
}

#[derive(Debug, Clone, Copy)]
struct NormalizedKey {
    usage: u16,
    down: bool,
    repeat: bool,
}

pub struct KeyRouter {
    cfg: KeyRouterConfig,
    captured: bool,
    tracker: ModTracker,
    /// Physical usages currently held by the local keyboard. This includes
    /// keys that were not sent because the remote pressed-key limit was full.
    physical_held: Vec<u16>,
    /// Ordinary usages that have a corresponding down at the remote. Kept in
    /// press order for the macOS command-release repair.
    pressed: Vec<u16>,
    chord: ChordState,
}

impl KeyRouter {
    /// 新建时处于「捕获」状态。
    pub fn new(cfg: KeyRouterConfig) -> Self {
        Self {
            cfg,
            captured: true,
            tracker: ModTracker::default(),
            physical_held: Vec::new(),
            pressed: Vec::new(),
            chord: ChordState::Idle,
        }
    }

    pub fn config(&self) -> KeyRouterConfig {
        self.cfg
    }

    /// 连上后知道 Host 系统时调用。只影响之后的按键；如果当前有按住的键，先返回 [Send(ReleaseAll)]。
    pub fn set_host(&mut self, host: HostOs, remap: ModifierRemap) -> Vec<KeyAction> {
        let actions = self.release_all();
        self.cfg.host = host;
        self.cfg.remap = remap;
        actions
    }

    pub fn captured(&self) -> bool {
        self.captured
    }

    /// 菜单或状态机改捕获状态。改成 false 且有按住的键 → [Send(ReleaseAll)]，并清状态。
    pub fn set_captured(&mut self, on: bool) -> Vec<KeyAction> {
        if on == self.captured {
            return Vec::new();
        }
        let actions = if on { Vec::new() } else { self.release_all() };
        self.captured = on;
        actions
    }

    pub fn on_key(&mut self, ev: KeyInput) -> Vec<KeyAction> {
        let events = self.normalize(ev);
        let mut actions = Vec::new();
        for event in events {
            self.process(event, &mut actions);
        }
        actions
    }

    /// 失焦 / 关窗 / 断线 / 变成观看者时调用。有按住的键 → [Send(ReleaseAll)] 并清状态，否则 []。
    pub fn release_all(&mut self) -> Vec<KeyAction> {
        let actions = if self.has_held() {
            vec![KeyAction::Send(InputMsg::ReleaseAll)]
        } else {
            Vec::new()
        };
        self.clear_state();
        actions
    }

    /// 菜单「发送组合键」：按 SessionCombo 的 JSON 名查 `combos::combo_inputs`；未知名 → None。不做 remap。
    pub fn combo(name: &str) -> Option<Vec<InputMsg>> {
        use rs_mobile::rscore::{combos, dto_core::SessionCombo};

        let combo: SessionCombo =
            serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()?;
        Some(combos::combo_inputs(combo))
    }

    fn normalize(&self, ev: KeyInput) -> Vec<NormalizedKey> {
        match ev {
            KeyInput::MacKey { vk, down, repeat } => mac_vk_to_hid(vk, self.cfg.mac_kind)
                .map(|usage| {
                    vec![NormalizedKey {
                        usage,
                        down,
                        repeat,
                    }]
                })
                .unwrap_or_default(),
            KeyInput::MacFlags { vk, flags } => match mac_flags_change(vk, flags) {
                MacFlagsChange::Modifier { usage, down } => vec![NormalizedKey {
                    usage,
                    down,
                    repeat: false,
                }],
                MacFlagsChange::CapsLockPressed => vec![
                    NormalizedKey {
                        usage: CAPS_LOCK,
                        down: true,
                        repeat: false,
                    },
                    NormalizedKey {
                        usage: CAPS_LOCK,
                        down: false,
                        repeat: false,
                    },
                ],
                MacFlagsChange::Ignored => Vec::new(),
            },
            KeyInput::WinKey {
                scan,
                extended,
                vk,
                down,
                repeat,
            } => win_scan_to_hid(scan, extended)
                .or_else(|| (vk == 0x13).then_some(0x48))
                .map(|usage| {
                    vec![NormalizedKey {
                        usage,
                        down,
                        repeat,
                    }]
                })
                .unwrap_or_default(),
        }
    }

    fn process(&mut self, event: NormalizedKey, actions: &mut Vec<KeyAction>) {
        if event.repeat && is_modifier(event.usage) {
            return;
        }
        let physical_empty_before = self.physical_held.is_empty();
        self.observe_chord(event, physical_empty_before);
        let physical_was_held = self.physical_held.contains(&event.usage);
        self.update_physical(event.usage, event.down);
        let last_command_up = self.cfg.client == ClientOs::Mac
            && !event.down
            && physical_was_held
            && matches!(event.usage, LEFT_META | RIGHT_META)
            && !self.physical_held.contains(&LEFT_META)
            && !self.physical_held.contains(&RIGHT_META);

        if self.captured {
            if last_command_up {
                self.flush_pressed(actions);
            }
            self.dispatch_captured(event, actions);
        }
        if last_command_up {
            // AppKit can omit ordinary key-ups while command is held. Treat
            // the repaired keys as released locally too, including keys that
            // exceeded the remote limit, so a later capture chord can start.
            self.physical_held.retain(|&usage| is_modifier(usage));
        }

        if self.finish_chord_if_empty() {
            self.finish_chord(actions);
        }
    }

    fn dispatch_captured(&mut self, event: NormalizedKey, actions: &mut Vec<KeyAction>) {
        let usage = remap_usage(event.usage, self.cfg.remap);
        let mods = if is_modifier(usage) {
            if event.down && self.tracker.mods().contains(mod_bit(usage)) {
                return;
            }
            self.tracker.apply(usage, event.down)
        } else {
            if event.down {
                if !self.pressed.contains(&usage) && !event.repeat {
                    if self.pressed.len() >= MAX_PRESSED {
                        return;
                    }
                    self.pressed.push(usage);
                }
            } else if let Some(index) = self.pressed.iter().position(|&held| held == usage) {
                self.pressed.remove(index);
            } else {
                return;
            }
            self.tracker.mods()
        };
        actions.push(KeyAction::Send(InputMsg::Key {
            usage,
            down: event.down,
            mods,
        }));
    }

    fn observe_chord(&mut self, event: NormalizedKey, physical_empty_before: bool) {
        if !event.down {
            return;
        }
        match self.chord {
            ChordState::Idle if physical_empty_before && is_chord_key(event.usage) => {
                self.chord = ChordState::Armed(chord_bit(event.usage));
            }
            ChordState::Armed(mut seen) if is_chord_key(event.usage) => {
                seen |= chord_bit(event.usage);
                self.chord = ChordState::Armed(seen);
            }
            ChordState::Armed(_) => self.chord = ChordState::Spoiled,
            ChordState::Spoiled | ChordState::Idle => {}
        }
    }

    fn finish_chord_if_empty(&mut self) -> bool {
        if !self.physical_held.is_empty() {
            return false;
        }
        let triggered = matches!(self.chord, ChordState::Armed(seen) if seen == 0b11);
        self.chord = ChordState::Idle;
        triggered
    }

    fn finish_chord(&mut self, actions: &mut Vec<KeyAction>) {
        self.clear_state();
        if self.captured {
            self.captured = false;
            actions.push(KeyAction::ReleaseCapture);
            actions.push(KeyAction::Send(InputMsg::ReleaseAll));
        } else {
            self.captured = true;
            actions.push(KeyAction::ResumeCapture);
        }
    }

    fn flush_pressed(&mut self, actions: &mut Vec<KeyAction>) {
        let mods = self.tracker.mods();
        let pressed = std::mem::take(&mut self.pressed);
        for usage in pressed {
            actions.push(KeyAction::Send(InputMsg::Key {
                usage,
                down: false,
                mods,
            }));
        }
    }

    fn update_physical(&mut self, usage: u16, down: bool) {
        if down {
            if !self.physical_held.contains(&usage) {
                self.physical_held.push(usage);
            }
        } else if let Some(index) = self.physical_held.iter().position(|&held| held == usage) {
            self.physical_held.remove(index);
        }
    }

    fn has_held(&self) -> bool {
        !self.physical_held.is_empty()
            || !self.pressed.is_empty()
            || self.tracker.mods() != Mods::NONE
    }

    fn clear_state(&mut self) {
        self.tracker.clear();
        self.physical_held.clear();
        self.pressed.clear();
        self.chord = ChordState::Idle;
    }
}

fn is_modifier(usage: u16) -> bool {
    (FIRST_MODIFIER..=LAST_MODIFIER).contains(&usage)
}

fn mod_bit(usage: u16) -> Mods {
    if is_modifier(usage) {
        Mods::from_bits(1 << (usage - FIRST_MODIFIER))
    } else {
        Mods::NONE
    }
}

fn is_chord_key(usage: u16) -> bool {
    usage == LEFT_CTRL || usage == LEFT_ALT
}

fn chord_bit(usage: u16) -> u8 {
    if usage == LEFT_CTRL {
        0b01
    } else {
        0b10
    }
}
