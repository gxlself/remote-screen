//! Pure frame gating for DXGI Desktop Duplication.

/// The fields of `DXGI_OUTDUPL_FRAME_INFO` that affect frame gating.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameInfoLite {
    pub last_present_time: i64,
    pub accumulated_frames: u32,
    pub total_metadata_buffer_size: u32,
    pub last_mouse_update_time: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameAction {
    Encode,
    Skip,
    PointerOnly,
}

/// Tracks whether an output duplication must encode its first present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameGate {
    fresh: bool,
}

impl FrameGate {
    pub fn new() -> Self {
        Self { fresh: true }
    }

    /// Re-arm the first-frame path after rebuilding an output duplication.
    pub fn reset(&mut self) {
        self.fresh = true;
    }

    pub fn decide(&mut self, info: &FrameInfoLite) -> FrameAction {
        if info.last_present_time != 0 {
            if self.fresh {
                self.fresh = false;
                FrameAction::Encode
            } else if info.total_metadata_buffer_size > 0 {
                FrameAction::Encode
            } else {
                FrameAction::Skip
            }
        } else if info.last_mouse_update_time != 0 {
            FrameAction::PointerOnly
        } else {
            FrameAction::Skip
        }
    }
}

impl Default for FrameGate {
    fn default() -> Self {
        Self::new()
    }
}
