//! RFB 3.8 (VNC) client for the built-in Mac Screen Sharing adapter (M3 V4).
//! Native RFB transport and decoding for desktop sessions.

pub mod auth;
pub mod client;
pub mod encodings;
pub mod keysym;
pub mod pixel;
pub mod proto;
pub mod surface;
pub mod testkit;
pub mod types;

pub use surface::{MemSurface, Screen, Surface};
pub use types::*;
