//! macOS input v2. Sequencing stays in host-core; only [`CgPoster`] posts events.

use host_core::inject::{keyseq, layout, scroll, scroll_gesture, text_plan};
use rsp_core::hid::InputMsg;

extern "C" {
    fn rs_kb_profile(layout_type: *mut u32, keyboard_type: *mut u32) -> i32;
    fn rs_input_key(vk: u16, kind: i32, flags: u64, autorepeat: i32, keyboard_type: u32) -> i32;
    fn rs_input_unicode(units: *const u16, len: u32) -> i32;
    fn rs_input_scroll2(
        dy: i32,
        dx: i32,
        scroll_phase: u32,
        momentum_phase: u32,
        continuous: i32,
    ) -> i32;
    fn rs_input_mouse_button2(button: i32, down: i32, x: f64, y: f64, clicks: i32);
    fn rs_input_mouse_move(x: f64, y: f64);
}

/// Replace this with a recorder to exercise the injector without sending input.
pub trait Poster: Send {
    fn key(&mut self, ev: keyseq::MacKeyEvent, keyboard_type: u32);
    fn unicode(&mut self, units: &[u16]);
    /// An unmodified down/up pair, used for Return and Tab in committed text.
    fn tap(&mut self, vk: u16, keyboard_type: u32);
    fn scroll(&mut self, ev: scroll_gesture::MacScrollEvent);
    fn mouse_button(&mut self, button: u8, down: bool, x: f32, y: f32, clicks: u8);
    fn mouse_move(&mut self, x: f32, y: f32);
}

/// Posts through shim.m, whose `post()` marks every event with `0x5253`.
///
/// Callers must serialize native posting and display selection (MacPlatform
/// does this with INPUT_LOCK). No Accessibility permission is requested here.
pub struct CgPoster;

impl Poster for CgPoster {
    fn key(&mut self, ev: keyseq::MacKeyEvent, keyboard_type: u32) {
        let kind = match ev.kind {
            keyseq::MacKeyKind::Down => 0,
            keyseq::MacKeyKind::Up => 1,
            keyseq::MacKeyKind::FlagsChanged => 2,
        };
        // SAFETY: primitive event fields; the shim owns and releases the CGEvent.
        let status = unsafe {
            rs_input_key(
                ev.vk,
                kind,
                ev.flags,
                i32::from(ev.autorepeat),
                keyboard_type,
            )
        };
        check_status("key", status);
    }

    fn unicode(&mut self, units: &[u16]) {
        if units.is_empty() {
            return;
        }
        let Ok(len) = u32::try_from(units.len()) else {
            tracing::warn!(
                units = units.len(),
                "Unicode input exceeds shim length limit"
            );
            return;
        };
        // SAFETY: the shim reads exactly len units synchronously and retains no pointer.
        let status = unsafe { rs_input_unicode(units.as_ptr(), len) };
        check_status("unicode", status);
    }

    fn tap(&mut self, vk: u16, keyboard_type: u32) {
        for kind in [keyseq::MacKeyKind::Down, keyseq::MacKeyKind::Up] {
            self.key(
                keyseq::MacKeyEvent {
                    kind,
                    vk,
                    flags: 0,
                    autorepeat: false,
                },
                keyboard_type,
            );
        }
    }

    fn scroll(&mut self, ev: scroll_gesture::MacScrollEvent) {
        let scroll = ev.scroll;
        // SAFETY: host-core supplies pixel deltas and CoreGraphics phase values.
        let status = unsafe {
            rs_input_scroll2(
                scroll.dy,
                scroll.dx,
                scroll.scroll_phase,
                scroll.momentum_phase,
                i32::from(ev.continuous),
            )
        };
        check_status("scroll", status);
    }

    fn mouse_button(&mut self, button: u8, down: bool, x: f32, y: f32, clicks: u8) {
        // SAFETY: all arguments are primitive fields; the shim owns its CGEvents.
        unsafe {
            rs_input_mouse_button2(
                i32::from(button),
                i32::from(down),
                f64::from(x),
                f64::from(y),
                i32::from(clicks),
            )
        };
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        // SAFETY: primitive coordinates; the shim owns and releases the CGEvent.
        unsafe { rs_input_mouse_move(f64::from(x), f64::from(y)) };
    }
}

fn check_status(event: &'static str, status: i32) {
    if status != 0 {
        // Never include keycodes or text in injection diagnostics.
        tracing::warn!(event, status, "CoreGraphics input creation failed");
    }
}

/// Raw `KBGetLayoutType(LMGetKbdType())` and `LMGetKbdType()` for diagnostics.
/// Returns None when Carbon cannot identify a keyboard; no events are posted.
pub fn native_keyboard_profile() -> Option<(u32, u32)> {
    let (mut layout_type, mut keyboard_type) = (0, 0);
    // SAFETY: both output pointers reference live u32s for this synchronous call.
    let status = unsafe { rs_kb_profile(&mut layout_type, &mut keyboard_type) };
    (status == 0).then_some((layout_type, keyboard_type))
}

fn detect_profile() -> layout::KeyboardProfile {
    match native_keyboard_profile() {
        Some((layout_type, keyboard_type)) => {
            layout::KeyboardProfile::detect(Some(layout_type), Some(keyboard_type))
        }
        None => layout::KeyboardProfile::detect(None, None),
    }
}

pub struct MacInjector<P: Poster = CgPoster> {
    poster: P,
    keys: keyseq::KeyTracker,
    scroll: scroll_gesture::ScrollGesture,
    profile: layout::KeyboardProfile,
}

impl MacInjector<CgPoster> {
    pub fn new() -> Self {
        Self::with_poster(CgPoster, detect_profile())
    }
}

impl Default for MacInjector<CgPoster> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Poster> MacInjector<P> {
    pub fn with_poster(poster: P, profile: layout::KeyboardProfile) -> Self {
        Self {
            poster,
            keys: keyseq::KeyTracker::new(),
            scroll: scroll_gesture::ScrollGesture::new(scroll::DEFAULT_PIXELS_PER_LINE),
            profile,
        }
    }

    pub fn inject(&mut self, msg: &InputMsg, now_us: u64) {
        match msg {
            InputMsg::Key { usage, down, mods } => {
                let profile = self.profile;
                for ev in self.keys.on_key(*usage, *down, *mods, &|usage| {
                    layout::resolve_vk(usage, &profile)
                }) {
                    self.poster.key(ev, profile.keyboard_type);
                }
            }
            InputMsg::Text(text) => {
                for step in text_plan::plan_text(text).steps {
                    match step {
                        text_plan::TextStep::Unicode(units) => self.poster.unicode(&units),
                        text_plan::TextStep::Key(vk) => {
                            self.poster.tap(vk, self.profile.keyboard_type);
                        }
                    }
                }
            }
            InputMsg::Scroll { .. } => {
                for ev in self.scroll.on_scroll(msg, now_us) {
                    self.poster.scroll(ev);
                }
            }
            InputMsg::MouseMove { x, y } => {
                self.poster.mouse_move(*x, *y);
            }
            InputMsg::MouseButton {
                button,
                down,
                x,
                y,
                clicks,
            } => {
                let button = *button as u8;
                self.poster.mouse_button(button, *down, *x, *y, *clicks);
            }
            InputMsg::ReleaseAll => self.release_all(),
            InputMsg::Touch { .. } => {} // Windows M2 only.
        }
    }

    pub fn poll(&mut self, now_us: u64) {
        for ev in self.scroll.poll(now_us) {
            self.poster.scroll(ev);
        }
    }

    pub fn release_all(&mut self) {
        for ev in self.keys.release_all() {
            self.poster.key(ev, self.profile.keyboard_type);
        }
        for ev in self.scroll.reset() {
            self.poster.scroll(ev);
        }
    }

    /// Refresh at session start. Release held input using the old profile first.
    pub fn refresh_profile(&mut self) {
        self.release_all();
        self.profile = detect_profile();
    }

    pub fn profile(&self) -> layout::KeyboardProfile {
        self.profile
    }

    pub fn poster(&self) -> &P {
        &self.poster
    }
}
