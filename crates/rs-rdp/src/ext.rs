//! Hooks between client.rs and the feature modules gfx, audio, clip and display
//! . client.rs calls every hook of every module, in the order gfx, audio,
//! clip, display; a feature card changes only its own module. Not part of the public API.
#![doc(hidden)]

use std::sync::{atomic::AtomicBool, Arc};

use tokio::sync::Notify;

use crate::types::{RdpAudioSink, RdpSink};

/// Shared state client.rs hands every hook for one connection attempt.
pub struct ExtCtx {
    pub sink: Arc<dyn RdpSink>,
    pub audio: Option<Arc<dyn RdpAudioSink>>,
    /// A module notifies it when it has messages queued outside `ActiveStage::process`
    /// (for example the clipboard backend); client.rs then calls every `pump`.
    pub wake: Arc<Notify>,
    /// Display only: channel traffic and acknowledgements continue while paused.
    pub background: Arc<AtomicBool>,
}
