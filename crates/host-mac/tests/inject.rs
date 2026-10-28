#![cfg(target_os = "macos")]

use host_core::inject::{
    keymap_mac::MacLayout,
    keyseq::{MacKeyEvent, MacKeyKind},
    layout::KeyboardProfile,
    scroll::MacScroll,
    scroll_gesture::MacScrollEvent,
};
use host_mac::inject::{MacInjector, Poster};
use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit, TouchPhase};

#[derive(Debug, PartialEq)]
enum Call {
    Key(MacKeyEvent, u32),
    Unicode(Vec<u16>),
    Tap(u16, u32),
    Scroll(MacScrollEvent),
    MouseButton(u8, bool, f32, f32, u8),
    MouseMove(f32, f32),
}

#[derive(Default)]
struct RecordingPoster(Vec<Call>);

impl Poster for RecordingPoster {
    fn key(&mut self, ev: MacKeyEvent, keyboard_type: u32) {
        self.0.push(Call::Key(ev, keyboard_type));
    }
    fn unicode(&mut self, units: &[u16]) {
        self.0.push(Call::Unicode(units.to_vec()));
    }
    fn tap(&mut self, vk: u16, keyboard_type: u32) {
        self.0.push(Call::Tap(vk, keyboard_type));
    }
    fn scroll(&mut self, ev: MacScrollEvent) {
        self.0.push(Call::Scroll(ev));
    }
    fn mouse_button(&mut self, button: u8, down: bool, x: f32, y: f32, clicks: u8) {
        self.0.push(Call::MouseButton(button, down, x, y, clicks));
    }
    fn mouse_move(&mut self, x: f32, y: f32) {
        self.0.push(Call::MouseMove(x, y));
    }
}

fn injector() -> MacInjector<RecordingPoster> {
    MacInjector::with_poster(RecordingPoster::default(), KeyboardProfile::default())
}

fn key(usage: u16, down: bool, mods: Mods) -> InputMsg {
    InputMsg::Key { usage, down, mods }
}

fn key_call(kind: MacKeyKind, vk: u16, flags: u64, keyboard_type: u32) -> Call {
    Call::Key(
        MacKeyEvent {
            kind,
            vk,
            flags,
            autorepeat: false,
        },
        keyboard_type,
    )
}

fn scroll(phase: ScrollPhase, momentum: ScrollPhase) -> InputMsg {
    InputMsg::Scroll {
        dx: 1.0,
        dy: -4.0,
        phase,
        momentum,
        unit: ScrollUnit::Pixel,
    }
}

fn scroll_call(phase: u32, momentum: u32, synthetic: bool) -> Call {
    Call::Scroll(MacScrollEvent {
        scroll: MacScroll {
            units_pixel: true,
            dx: if synthetic { 0 } else { 1 },
            dy: if synthetic { 0 } else { -4 },
            scroll_phase: phase,
            momentum_phase: momentum,
        },
        continuous: true,
        synthetic,
    })
}

#[test]
fn committed_text_becomes_unicode_and_real_return() {
    let mut injector = injector();
    injector.inject(&InputMsg::Text("你好\n".into()), 0);
    assert_eq!(
        injector.poster().0,
        [
            Call::Unicode("你好".encode_utf16().collect()),
            Call::Tap(0x24, 40)
        ]
    );
}

#[test]
fn ascii_text_uses_twenty_unit_chunks() {
    let mut injector = injector();
    injector.inject(&InputMsg::Text("a".repeat(45)), 0);
    assert_eq!(
        injector.poster().0,
        [
            Call::Unicode(vec![0x61; 20]),
            Call::Unicode(vec![0x61; 20]),
            Call::Unicode(vec![0x61; 5]),
        ]
    );
}

#[test]
fn text_preserves_emoji_and_tabs_while_command_is_held() {
    let mut injector = injector();
    injector.inject(&key(0xE3, true, Mods::LMETA), 0);
    injector.inject(&InputMsg::Text("你好🙂👨‍👩‍👧‍👦\r\n第二行\t".into()), 1);
    assert_eq!(
        &injector.poster().0[1..],
        [
            Call::Unicode("你好🙂👨‍👩‍👧‍👦".encode_utf16().collect()),
            Call::Tap(0x24, 40),
            Call::Unicode("第二行".encode_utf16().collect()),
            Call::Tap(0x30, 40),
        ]
    );
    injector.release_all();
    assert_eq!(
        injector.poster().0.last(),
        Some(&key_call(MacKeyKind::FlagsChanged, 0x37, 0, 40))
    );
}

#[test]
fn command_tab_matches_h9d_vector_and_native_keyboard_type() {
    let profile = KeyboardProfile::detect(Some(u32::from_be_bytes(*b"ANSI")), Some(47));
    let mut injector = MacInjector::with_poster(RecordingPoster::default(), profile);
    for (usage, down, mods) in [
        (0xE3, true, Mods::LMETA),
        (0x2B, true, Mods::LMETA),
        (0x2B, false, Mods::LMETA),
        (0x2B, true, Mods::LMETA),
        (0x2B, false, Mods::LMETA),
        (0xE3, false, Mods::NONE),
    ] {
        injector.inject(&key(usage, down, mods), 0);
    }
    assert_eq!(
        injector.poster().0,
        [
            key_call(MacKeyKind::FlagsChanged, 0x37, 0x100008, 47),
            key_call(MacKeyKind::Down, 0x30, 0x100008, 47),
            key_call(MacKeyKind::Up, 0x30, 0x100008, 47),
            key_call(MacKeyKind::Down, 0x30, 0x100008, 47),
            key_call(MacKeyKind::Up, 0x30, 0x100008, 47),
            key_call(MacKeyKind::FlagsChanged, 0x37, 0, 47),
        ]
    );
}

#[test]
fn mission_control_keeps_secondary_fn_and_numeric_pad_flags() {
    let mut injector = injector();
    injector.inject(&key(0x52, true, Mods::LCTRL), 0);
    injector.inject(&key(0x52, false, Mods::LCTRL), 1);
    injector.release_all();
    assert_eq!(
        injector.poster().0,
        [
            key_call(MacKeyKind::FlagsChanged, 0x3B, 0x40001, 40),
            key_call(MacKeyKind::Down, 0x7E, 0xA40001, 40),
            key_call(MacKeyKind::Up, 0x7E, 0xA40001, 40),
            key_call(MacKeyKind::FlagsChanged, 0x3B, 0, 40),
        ]
    );
}

#[test]
fn iso_position_is_swapped_exactly_once_and_headless_falls_back() {
    for (profile, vk) in [
        (KeyboardProfile::new(MacLayout::Iso), 0x0A),
        (KeyboardProfile::new(MacLayout::Ansi), 0x32),
        (KeyboardProfile::detect(None, None), 0x32),
    ] {
        let mut injector = MacInjector::with_poster(RecordingPoster::default(), profile);
        injector.inject(&key(0x35, true, Mods::NONE), 0);
        injector.inject(&key(0x35, false, Mods::NONE), 1);
        assert_eq!(injector.profile(), profile);
        assert_eq!(
            injector.poster().0,
            [
                key_call(MacKeyKind::Down, vk, 0, profile.keyboard_type),
                key_call(MacKeyKind::Up, vk, 0, profile.keyboard_type),
            ]
        );
    }
}

#[test]
fn trackpad_and_momentum_preserve_coregraphics_phase_values() {
    let mut injector = injector();
    let mut now = 1_000_000;
    let mut expected = Vec::new();
    for (phase, momentum, count, cg_phase, cg_momentum) in [
        (ScrollPhase::Began, ScrollPhase::None, 1, 1, 0),
        (ScrollPhase::Changed, ScrollPhase::None, 20, 2, 0),
        (ScrollPhase::Ended, ScrollPhase::None, 1, 4, 0),
        (ScrollPhase::None, ScrollPhase::Began, 1, 0, 1),
        (ScrollPhase::None, ScrollPhase::Changed, 30, 0, 2),
        (ScrollPhase::None, ScrollPhase::Ended, 1, 0, 3),
    ] {
        for _ in 0..count {
            injector.inject(&scroll(phase, momentum), now);
            expected.push(scroll_call(cg_phase, cg_momentum, false));
            now += 16_000;
        }
    }
    injector.poll(now + 300_000);
    assert_eq!(injector.poster().0, expected);
}

#[test]
fn polling_closes_a_cut_scroll_at_three_hundred_milliseconds() {
    let mut injector = injector();
    injector.inject(&scroll(ScrollPhase::Began, ScrollPhase::None), 1_000_000);
    injector.inject(&scroll(ScrollPhase::Changed, ScrollPhase::None), 1_016_000);
    injector.poll(1_315_999);
    assert_eq!(injector.poster().0.len(), 2);
    injector.poll(1_316_000);
    injector.poll(2_000_000);
    assert_eq!(
        injector.poster().0,
        [
            scroll_call(1, 0, false),
            scroll_call(2, 0, false),
            scroll_call(4, 0, true)
        ]
    );
}

#[test]
fn release_all_closes_keys_modifiers_and_scroll_once() {
    let mut injector = injector();
    injector.inject(&key(0xE3, true, Mods::LMETA), 0);
    injector.inject(&key(0x04, true, Mods::LMETA), 1);
    injector.inject(&scroll(ScrollPhase::Began, ScrollPhase::None), 2);
    injector.inject(&InputMsg::ReleaseAll, 3);
    injector.release_all();
    injector.poll(400_000);
    assert_eq!(
        &injector.poster().0[3..],
        [
            key_call(MacKeyKind::Up, 0, 0x100008, 40),
            key_call(MacKeyKind::FlagsChanged, 0x37, 0, 40),
            scroll_call(4, 0, true),
        ]
    );
}

#[test]
fn side_buttons_are_forwarded_with_the_protocol_button_number() {
    let mut injector = injector();
    for button in [
        Button::Left,
        Button::Right,
        Button::Middle,
        Button::Back,
        Button::Forward,
    ] {
        injector.inject(
            &InputMsg::MouseButton {
                button,
                down: true,
                x: 0.2,
                y: 0.3,
                clicks: 2,
            },
            0,
        );
    }
    assert_eq!(
        injector.poster().0[3],
        Call::MouseButton(3, true, 0.2, 0.3, 2)
    );
    assert_eq!(
        injector.poster().0[4],
        Call::MouseButton(4, true, 0.2, 0.3, 2)
    );
    injector.inject(&InputMsg::MouseMove { x: 0.8, y: 0.9 }, 1);
    assert_eq!(injector.poster().0[5], Call::MouseMove(0.8, 0.9));
}

#[test]
fn macos_ignores_touch() {
    let mut injector = injector();
    injector.inject(
        &InputMsg::Touch {
            id: 0,
            phase: TouchPhase::Began,
            x: 0.5,
            y: 0.5,
            pressure: 1.0,
        },
        0,
    );
    assert!(injector.poster().0.is_empty());
}
