use std::collections::BTreeMap;

use host_core::inject::keyseq::{
    implicit_flags, KeyTracker, MacKeyEvent, MacKeyKind, FLAG_ALPHA_SHIFT, FLAG_NUMERIC_PAD,
    FLAG_SECONDARY_FN, MAX_PRESSED, MOD_ORDER, VK_CAPS_LOCK,
};
use host_core::inject::{hid_to_vk, MacLayout};
use rsp_core::hid::Mods;

fn key(tracker: &mut KeyTracker, usage: u16, down: bool, mods: Mods) -> Vec<MacKeyEvent> {
    let resolve = |u| hid_to_vk(u, MacLayout::Ansi);
    tracker.on_key(usage, down, mods, &resolve)
}

fn fc(vk: u16, flags: u64) -> MacKeyEvent {
    MacKeyEvent {
        kind: MacKeyKind::FlagsChanged,
        vk,
        flags,
        autorepeat: false,
    }
}

fn down(vk: u16, flags: u64) -> MacKeyEvent {
    MacKeyEvent {
        kind: MacKeyKind::Down,
        vk,
        flags,
        autorepeat: false,
    }
}

fn repeat(vk: u16, flags: u64) -> MacKeyEvent {
    MacKeyEvent {
        kind: MacKeyKind::Down,
        vk,
        flags,
        autorepeat: true,
    }
}

fn up(vk: u16, flags: u64) -> MacKeyEvent {
    MacKeyEvent {
        kind: MacKeyKind::Up,
        vk,
        flags,
        autorepeat: false,
    }
}

const LCTRL: Mods = Mods::LCTRL;
const LSHIFT: Mods = Mods::LSHIFT;
const LALT: Mods = Mods::LALT;
const LMETA: Mods = Mods::LMETA;
const RMETA: Mods = Mods::RMETA;
const NONE: Mods = Mods::NONE;

#[test]
fn command_tab_twice_with_explicit_modifier_keys() {
    let mut tracker = KeyTracker::new();
    let mut events = Vec::new();
    for (usage, is_down, mods) in [
        (0xE3, true, LMETA),
        (0x2B, true, LMETA),
        (0x2B, false, LMETA),
        (0x2B, true, LMETA),
        (0x2B, false, LMETA),
        (0xE3, false, NONE),
    ] {
        events.extend(key(&mut tracker, usage, is_down, mods));
    }

    assert_eq!(
        events,
        vec![
            fc(0x37, 0x100008),
            down(0x30, 0x100008),
            up(0x30, 0x100008),
            down(0x30, 0x100008),
            up(0x30, 0x100008),
            fc(0x37, 0),
        ]
    );
}

#[test]
fn command_tab_with_command_only_in_mods_and_release_all() {
    let mut tracker = KeyTracker::new();
    assert_eq!(
        key(&mut tracker, 0x2B, true, LMETA),
        vec![fc(0x37, 0x100008), down(0x30, 0x100008)]
    );
    assert_eq!(
        key(&mut tracker, 0x2B, false, LMETA),
        vec![up(0x30, 0x100008)]
    );
    assert_eq!(tracker.release_all(), vec![fc(0x37, 0)]);

    let mut tracker = KeyTracker::new();
    assert_eq!(
        key(&mut tracker, 0x2B, true, LMETA),
        vec![fc(0x37, 0x100008), down(0x30, 0x100008)]
    );
    assert_eq!(
        key(&mut tracker, 0x2B, false, LMETA),
        vec![up(0x30, 0x100008)]
    );
    assert_eq!(
        key(&mut tracker, 0x04, true, NONE),
        vec![fc(0x37, 0), down(0x00, 0)]
    );
}

#[test]
fn mission_control_control_up_carries_secondary_fn() {
    let mut tracker = KeyTracker::new();
    let mut events = Vec::new();
    for (usage, is_down, mods) in [
        (0xE0, true, LCTRL),
        (0x52, true, LCTRL),
        (0x52, false, LCTRL),
        (0xE0, false, NONE),
    ] {
        events.extend(key(&mut tracker, usage, is_down, mods));
    }

    assert_eq!(
        events,
        vec![
            fc(0x3B, 0x40001),
            down(0x7E, 0xA40001),
            up(0x7E, 0xA40001),
            fc(0x3B, 0),
        ]
    );
}

#[test]
fn system_shortcut_vectors_have_expected_key_flags() {
    type KeyInput = (u16, bool, Mods);
    type ShortcutVector = (&'static str, Vec<KeyInput>, Vec<MacKeyEvent>);

    let vectors: Vec<ShortcutVector> = vec![
        (
            "cmdSpace",
            vec![(0xE3, true, LMETA), (0x2C, true, LMETA)],
            vec![fc(0x37, 0x100008), down(0x31, 0x100008)],
        ),
        (
            "ctrlCmdQ",
            vec![
                (0xE0, true, LCTRL),
                (0xE3, true, LCTRL | LMETA),
                (0x14, true, LCTRL | LMETA),
            ],
            vec![fc(0x3B, 0x40001), fc(0x37, 0x140009), down(0x0C, 0x140009)],
        ),
        (
            "cmdShift4",
            vec![
                (0xE3, true, LMETA),
                (0xE1, true, LMETA | LSHIFT),
                (0x21, true, LMETA | LSHIFT),
            ],
            vec![fc(0x37, 0x100008), fc(0x38, 0x12000A), down(0x15, 0x12000A)],
        ),
        (
            "cmdOptionEscape",
            vec![
                (0xE3, true, LMETA),
                (0xE2, true, LMETA | LALT),
                (0x29, true, LMETA | LALT),
            ],
            vec![fc(0x37, 0x100008), fc(0x3A, 0x180028), down(0x35, 0x180028)],
        ),
    ];

    for (name, inputs, expected) in vectors {
        let mut tracker = KeyTracker::new();
        let mut events = Vec::new();
        for (usage, is_down, mods) in inputs {
            events.extend(key(&mut tracker, usage, is_down, mods));
        }
        assert_eq!(events, expected, "{name}");
    }
}

#[test]
fn modifier_sync_follows_mod_order() {
    let mut tracker = KeyTracker::new();
    let mods = LCTRL | LSHIFT | LALT | LMETA;
    let events = key(&mut tracker, 0x2C, true, mods);
    assert_eq!(
        events,
        vec![
            fc(0x3B, 0x40001),
            fc(0x38, 0x60003),
            fc(0x3A, 0xE0023),
            fc(0x37, 0x1E002B),
            down(0x31, 0x1E002B),
        ]
    );
}

#[test]
fn right_side_modifiers_keep_their_identity() {
    let mut tracker = KeyTracker::new();
    assert_eq!(
        key(&mut tracker, 0xE7, true, RMETA),
        vec![fc(0x36, 0x100010)]
    );

    assert_eq!(
        key(&mut tracker, 0xE3, true, RMETA | LMETA),
        vec![fc(0x37, 0x100018)]
    );
    assert_eq!(
        key(&mut tracker, 0xE3, false, RMETA),
        vec![fc(0x37, 0x100010)]
    );
    assert_eq!(tracker.mods(), RMETA);
    assert_eq!(tracker.release_all(), vec![fc(0x36, 0)]);
}

#[test]
fn mods_can_add_a_modifier_without_a_modifier_key_event() {
    let mut tracker = KeyTracker::new();
    assert_eq!(
        key(&mut tracker, 0xE0, true, LCTRL),
        vec![fc(0x3B, 0x40001)]
    );
    assert_eq!(
        key(&mut tracker, 0x04, true, LCTRL | LSHIFT),
        vec![fc(0x38, 0x60003), down(0x00, 0x60003)]
    );
}

#[test]
fn modifier_down_wins_over_contradictory_mods_and_repeats_are_empty() {
    let mut tracker = KeyTracker::new();
    assert_eq!(
        key(&mut tracker, 0xE3, true, NONE),
        vec![fc(0x37, 0x100008)]
    );
    assert!(tracker.mods().contains(LMETA));
    assert!(key(&mut tracker, 0xE3, true, LMETA).is_empty());
}

#[test]
fn held_ordinary_key_repeats_without_growing_pressed() {
    let mut tracker = KeyTracker::new();
    assert_eq!(key(&mut tracker, 0x04, true, NONE), vec![down(0x00, 0)]);
    assert_eq!(key(&mut tracker, 0x04, true, NONE), vec![repeat(0x00, 0)]);
    assert_eq!(tracker.pressed(), 1);
}

#[test]
fn stray_unknown_and_overflow_inputs_are_counted() {
    let mut tracker = KeyTracker::new();
    assert!(key(&mut tracker, 0x04, false, NONE).is_empty());
    assert!(key(&mut tracker, 0x03, true, NONE).is_empty());
    assert_eq!(tracker.stats().stray_up, 1);
    assert_eq!(tracker.stats().unknown_usage, 1);

    let mut tracker = KeyTracker::new();
    let last = 0x04 + MAX_PRESSED as u16;
    for usage in 0x04..=last {
        let events = key(&mut tracker, usage, true, NONE);
        if usage == last {
            assert!(events.is_empty());
        } else {
            assert_eq!(events.len(), 1);
        }
    }
    assert_eq!(tracker.pressed(), MAX_PRESSED);
    assert_eq!(tracker.stats().overflow, 1);
}

#[test]
fn caps_lock_toggles_only_on_down_and_changes_flags() {
    let mut tracker = KeyTracker::new();
    assert_eq!(
        key(&mut tracker, 0x39, true, NONE),
        vec![fc(VK_CAPS_LOCK, FLAG_ALPHA_SHIFT)]
    );
    assert!(tracker.caps_lock());
    assert!(key(&mut tracker, 0x39, false, NONE).is_empty());
    assert_eq!(
        key(&mut tracker, 0x04, true, NONE),
        vec![down(0x00, FLAG_ALPHA_SHIFT)]
    );
    assert_eq!(
        key(&mut tracker, 0x39, true, NONE),
        vec![fc(VK_CAPS_LOCK, 0)]
    );
    assert!(!tracker.caps_lock());
    let _ = tracker.release_all();
    assert!(!tracker.caps_lock());

    let mut tracker = KeyTracker::new();
    let _ = key(&mut tracker, 0x39, true, NONE);
    assert!(tracker.caps_lock());
    let _ = tracker.release_all();
    assert!(tracker.caps_lock());
}

#[test]
fn implicit_flags_match_every_documented_virtual_key() {
    let arrow_vks = [0x7B, 0x7C, 0x7D, 0x7E];
    let fn_vks = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71,
        0x6A, 0x40, 0x4F, 0x50, 0x5A, 0x72, 0x73, 0x74, 0x75, 0x77, 0x79,
    ];
    let keypad_vks = [
        0x41, 0x43, 0x45, 0x47, 0x4B, 0x4C, 0x4E, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
        0x59, 0x5B, 0x5C, 0x5F,
    ];

    for vk in 0..=u16::MAX {
        let expected = if arrow_vks.contains(&vk) {
            FLAG_NUMERIC_PAD | FLAG_SECONDARY_FN
        } else if fn_vks.contains(&vk) {
            FLAG_SECONDARY_FN
        } else if keypad_vks.contains(&vk) {
            FLAG_NUMERIC_PAD
        } else {
            0
        };
        assert_eq!(implicit_flags(vk), expected, "vk {vk:#06x}");
    }

    let cases = [
        (0x59, 0x53, FLAG_NUMERIC_PAD),                     // Keypad1
        (0x3E, 0x60, FLAG_SECONDARY_FN),                    // F5
        (0x4A, 0x73, FLAG_SECONDARY_FN),                    // Home
        (0x50, 0x7B, FLAG_NUMERIC_PAD | FLAG_SECONDARY_FN), // Left arrow
        (0x04, 0x00, 0),                                    // A
    ];
    for (usage, vk, flags) in cases {
        let mut tracker = KeyTracker::new();
        assert_eq!(key(&mut tracker, usage, true, NONE), vec![down(vk, flags)]);
    }
}

#[test]
fn release_all_reverses_pressed_keys_then_modifiers() {
    let mut tracker = KeyTracker::new();
    let both = LCTRL | LSHIFT;
    assert_eq!(
        key(&mut tracker, 0xE0, true, LCTRL),
        vec![fc(0x3B, 0x40001)]
    );
    assert_eq!(key(&mut tracker, 0xE1, true, both), vec![fc(0x38, 0x60003)]);
    assert_eq!(
        key(&mut tracker, 0x04, true, both),
        vec![down(0x00, 0x60003)]
    );
    assert_eq!(
        key(&mut tracker, 0x05, true, both),
        vec![down(0x0B, 0x60003)]
    );

    assert_eq!(
        tracker.release_all(),
        vec![
            up(0x0B, 0x60003),
            up(0x00, 0x60003),
            fc(0x38, 0x40001),
            fc(0x3B, 0),
        ]
    );
    assert_eq!(tracker.pressed(), 0);
    assert_eq!(tracker.mods(), NONE);
    assert!(tracker.release_all().is_empty());
}

struct XorShift32(u32);

impl XorShift32 {
    fn next(&mut self) -> u32 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.0 = value;
        value
    }

    fn below(&mut self, bound: u32) -> u32 {
        self.next() % bound
    }
}

#[test]
fn random_sequences_balance_every_down_with_its_up() {
    const USAGES: [u16; 32] = [
        0x03, 0x04, 0x05, 0x06, 0x29, 0x2B, 0x2C, 0x31, 0x32, 0x39, 0x3A, 0x3E, 0x49, 0x4A, 0x50,
        0x52, 0x59, 0x5B, 0x67, 0x70, 0x73, 0x75, 0x7F, 0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6,
        0xE7, 0xFFFF,
    ];

    let mut random = XorShift32(0x6D2B_79F5);
    let mut tracker = KeyTracker::new();
    let mut events = Vec::new();
    for _ in 0..2000 {
        let usage = USAGES[random.below(USAGES.len() as u32) as usize];
        let is_down = random.next() & 1 == 0;
        let mods = Mods::from_bits(random.next() as u8);
        events.extend(key(&mut tracker, usage, is_down, mods));
    }
    assert!(tracker.pressed() > 0);
    events.extend(tracker.release_all());

    let last = events.last().expect("random run produced no events");
    assert!(
        last.flags == 0 || last.flags == FLAG_ALPHA_SHIFT,
        "last flags {:#x}",
        last.flags
    );

    let mut downs = BTreeMap::<u16, usize>::new();
    let mut ups = BTreeMap::<u16, usize>::new();
    for event in events {
        match event.kind {
            MacKeyKind::Down if !event.autorepeat => *downs.entry(event.vk).or_default() += 1,
            MacKeyKind::Up => *ups.entry(event.vk).or_default() += 1,
            MacKeyKind::Down | MacKeyKind::FlagsChanged => {}
        }
    }
    assert_eq!(downs, ups);
    assert_eq!(tracker.pressed(), 0);
    assert_eq!(tracker.mods(), NONE);
}

#[test]
fn modifier_order_is_the_documented_hid_order() {
    assert_eq!(
        MOD_ORDER,
        [
            Mods::LCTRL,
            Mods::LSHIFT,
            Mods::LALT,
            Mods::LMETA,
            Mods::RCTRL,
            Mods::RSHIFT,
            Mods::RALT,
            Mods::RMETA,
        ]
    );
}
