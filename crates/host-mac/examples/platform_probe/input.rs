//! Input probes deliberately print counts and state, never the injected text.

use std::time::Duration;

use host_core::{inject::layout::KeyboardProfile, platform::Platform};
use host_mac::{inject::native_keyboard_profile, MacPlatform};
use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit};

pub enum Action {
    Text(String),
    Combo(Combo),
    Keys(Vec<u16>),
    ScrollDemo,
    ScrollCut,
    Button(u8),
}

pub enum Combo {
    CmdTab,
    CmdSpace,
    MissionControl,
    AppWindows,
    Lock,
    Screenshot,
}

impl Combo {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "cmd-tab" => Self::CmdTab,
            "cmd-space" => Self::CmdSpace,
            "mission-control" => Self::MissionControl,
            "app-windows" => Self::AppWindows,
            "lock" => Self::Lock,
            "screenshot" => Self::Screenshot,
            _ => return None,
        })
    }

    fn keys(&self) -> (&'static [(u16, Mods)], u16) {
        const COMMAND: &[(u16, Mods)] = &[(0xE3, Mods::LMETA)];
        const CONTROL: &[(u16, Mods)] = &[(0xE0, Mods::LCTRL)];
        match self {
            Self::CmdTab => (COMMAND, 0x2B),
            Self::CmdSpace => (COMMAND, 0x2C),
            Self::MissionControl => (CONTROL, 0x52),
            Self::AppWindows => (CONTROL, 0x51),
            Self::Lock => (&[(0xE0, Mods::LCTRL), (0xE3, Mods::LMETA)], 0x14),
            Self::Screenshot => (&[(0xE3, Mods::LMETA), (0xE1, Mods::LSHIFT)], 0x21),
        }
    }
}

pub fn parse_keys(value: &str) -> Option<Vec<u16>> {
    value
        .split(',')
        .map(|part| {
            let part = part.trim();
            let part = part
                .strip_prefix("0x")
                .or_else(|| part.strip_prefix("0X"))
                .unwrap_or(part);
            u16::from_str_radix(part, 16).ok()
        })
        .collect()
}

pub fn print_profile() {
    let raw = native_keyboard_profile();
    let profile = KeyboardProfile::detect(raw.map(|(layout, _)| layout), raw.map(|(_, kbd)| kbd));
    match raw {
        Some((layout, kbd)) => {
            println!("KBGetLayoutType=0x{layout:08x}, LMGetKbdType={kbd} (0x{kbd:02x})");
        }
        None => println!("Carbon keyboard profile unavailable; using headless fallback"),
    }
    println!(
        "layout={:?}, keyboard_type={}, iso_position_swap={}",
        profile.layout, profile.keyboard_type, profile.iso_position_swap
    );
}

pub async fn run(platform: &MacPlatform, actions: &[Action]) {
    let Some(display) = platform
        .displays()
        .into_iter()
        .find(|display| display.primary)
    else {
        eprintln!("input probe needs a primary display");
        return;
    };
    platform.refresh_keyboard_profile();
    println!("{} input probe(s); switch to the target app", actions.len());
    for seconds in (1..=3).rev() {
        println!("input in {seconds}...");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let send = |msg| platform.inject_msg(&msg, &display);
    for action in actions {
        match action {
            Action::Text(text) => {
                send(InputMsg::Text(text.clone()));
                println!("text sent: {} UTF-16 units", text.encode_utf16().count());
            }
            Action::Combo(combo) => {
                let (modifiers, usage) = combo.keys();
                let mut mods = Mods::NONE;
                for &(usage, modifier) in modifiers {
                    mods |= modifier;
                    send(InputMsg::Key {
                        usage,
                        down: true,
                        mods,
                    });
                    key_pause().await;
                }
                send(InputMsg::Key {
                    usage,
                    down: true,
                    mods,
                });
                key_pause().await;
                send(InputMsg::Key {
                    usage,
                    down: false,
                    mods,
                });
                // Keep the switcher visible briefly before releasing Command.
                tokio::time::sleep(Duration::from_millis(500)).await;
                for &(usage, modifier) in modifiers.iter().rev() {
                    mods = Mods::from_bits(mods.bits() & !modifier.bits());
                    send(InputMsg::Key {
                        usage,
                        down: false,
                        mods,
                    });
                    key_pause().await;
                }
                println!("combo sent");
            }
            Action::Keys(keys) => {
                for &usage in keys {
                    send(InputMsg::Key {
                        usage,
                        down: true,
                        mods: Mods::NONE,
                    });
                    key_pause().await;
                    send(InputMsg::Key {
                        usage,
                        down: false,
                        mods: Mods::NONE,
                    });
                    key_pause().await;
                }
                println!("{} HID key tap(s) sent", keys.len());
            }
            Action::ScrollDemo | Action::ScrollCut => {
                let cut = matches!(action, Action::ScrollCut);
                send(scroll(ScrollPhase::Began, ScrollPhase::None, 0.0));
                for _ in 0..if cut { 5 } else { 20 } {
                    frame_pause().await;
                    send(scroll(ScrollPhase::Changed, ScrollPhase::None, -12.0));
                }
                if cut {
                    println!("scroll input stopped; waiting for the 100 ms poll to close it");
                    // Do not release_all here: that would mask a broken timeout.
                    tokio::time::sleep(Duration::from_millis(450)).await;
                } else {
                    frame_pause().await;
                    send(scroll(ScrollPhase::Ended, ScrollPhase::None, 0.0));
                    frame_pause().await;
                    send(scroll(ScrollPhase::None, ScrollPhase::Began, -12.0));
                    for step in 1..=30 {
                        frame_pause().await;
                        let dy = -12.0 * (1.0 - step as f32 / 31.0);
                        send(scroll(ScrollPhase::None, ScrollPhase::Changed, dy));
                    }
                    frame_pause().await;
                    send(scroll(ScrollPhase::None, ScrollPhase::Ended, 0.0));
                    println!("scroll gesture and momentum sent");
                }
            }
            Action::Button(button) => {
                let button = if *button == 3 {
                    Button::Back
                } else {
                    Button::Forward
                };
                let (x, y) = host_mac::cursor_position();
                for down in [true, false] {
                    send(InputMsg::MouseButton {
                        button,
                        down,
                        x,
                        y,
                        clicks: 1,
                    });
                    key_pause().await;
                }
                println!("side button tap sent");
            }
        }
    }
}

fn scroll(phase: ScrollPhase, momentum: ScrollPhase, dy: f32) -> InputMsg {
    InputMsg::Scroll {
        dx: 0.0,
        dy,
        phase,
        momentum,
        unit: ScrollUnit::Pixel,
    }
}

async fn key_pause() {
    tokio::time::sleep(Duration::from_millis(80)).await;
}

async fn frame_pause() {
    tokio::time::sleep(Duration::from_millis(16)).await;
}
