//! Pure input conversions for the macOS injector, also available on Linux.
//!
//! These helpers never inspect keyboard layouts or call platform APIs. The
//! platform layer chooses the physical layout and posts the resulting events.

pub mod keymap_mac;
pub mod keymap_win;
pub mod keyseq;
pub mod layout;
pub mod scroll;
pub mod scroll_gesture;
pub mod text;
pub mod text_plan;
pub mod win_plan;

pub use keymap_mac::{
    hid_to_vk, modifier_usage_to_vk, mods_to_cg_flags, vk_is_modifier, MacLayout,
};
pub use scroll::{to_mac_scroll, MacScroll, ScrollAccumulator, DEFAULT_PIXELS_PER_LINE};
pub use text::utf16_chunks;
