//! Deterministic, pixel-free platform for host development on any OS.

pub mod clip;
pub mod faultnet;
pub mod harness;
pub mod platform;

pub use platform::{FakePlatform, SyntheticFrame};

pub mod audio;
pub mod vdisplay;
pub mod viewport;
