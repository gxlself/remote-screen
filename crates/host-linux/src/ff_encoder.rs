//! FFmpeg hardware encoders for the Linux encoding worker.

use std::path::PathBuf;
use std::sync::Arc;

use crate::enc_worker::{BackendError, BackendFactory, EncBackend, EncPacket, SrcSize};
use crate::ff_model::{Candidate, EncParams, LinuxBackend, RateCost};
use crate::ffmpeg::{FfEncoder, FfError, FfLib, FfOpenReq};
use crate::frame::LinuxFrame;

pub struct FfFactory {
    lib: Arc<FfLib>,
    backend: LinuxBackend,
    render_node: Option<PathBuf>,
}

impl FfFactory {
    /// Create a factory for a VA-API or NVENC candidate. An OpenH264 candidate
    /// is accepted, but opening it always returns [`BackendError::Unsupported`].
    pub fn new(lib: Arc<FfLib>, cand: &Candidate) -> Self {
        Self {
            lib,
            backend: cand.backend,
            render_node: cand.render_node.clone(),
        }
    }
}

impl BackendFactory for FfFactory {
    fn name(&self) -> &'static str {
        self.backend.name()
    }

    fn open(&self, params: &EncParams, src: SrcSize) -> Result<Box<dyn EncBackend>, BackendError> {
        let encoder = FfEncoder::open(
            &self.lib,
            &FfOpenReq {
                backend: self.backend,
                params: *params,
                src_width: src.width,
                src_height: src.height,
                render_node: self.render_node.as_deref(),
            },
        )
        .map_err(backend_error)?;
        Ok(Box::new(FfBackend {
            encoder,
            backend: self.backend,
        }))
    }
}

struct FfBackend {
    encoder: FfEncoder,
    backend: LinuxBackend,
}

// SAFETY: FfEncoder exclusively owns its FFmpeg contexts, frames and packets;
// these resources may move between threads but must not be used concurrently.
// This private wrapper never exposes the encoder, all operations require &mut
// self, and its retained Arc<FfLib> keeps every library loaded through teardown.
// Moving the probed backend to EncWorker transfers ownership; it does not share
// the native contexts. FfBackend remains !Sync.
unsafe impl Send for FfBackend {}

impl EncBackend for FfBackend {
    fn name(&self) -> &'static str {
        self.backend.name()
    }

    fn rate_cost(&self) -> RateCost {
        self.backend.rate_cost()
    }

    fn encode(
        &mut self,
        frame: &LinuxFrame,
        pts: i64,
        idr: bool,
    ) -> Result<Vec<EncPacket>, BackendError> {
        self.encoder
            .encode(frame, pts, idr)
            .map(|packets| {
                packets
                    .into_iter()
                    .map(|packet| EncPacket {
                        data: packet.data,
                        pts: packet.pts,
                        keyframe: packet.keyframe,
                    })
                    .collect()
            })
            .map_err(backend_error)
    }

    fn set_rate(&mut self, bitrate_bps: u64, fps: u32) -> Result<(), BackendError> {
        self.encoder
            .set_rate(bitrate_bps, fps)
            .map_err(backend_error)
    }
}

fn backend_error(error: FfError) -> BackendError {
    match error {
        FfError::Load(_) | FfError::Abi(_) | FfError::Unsupported(_) => {
            BackendError::Unsupported(error.to_string())
        }
        FfError::Failed { .. } => BackendError::Failed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use host_core::ffmpeg_abi::AbiError;

    #[test]
    fn unavailable_backends_are_not_retried() {
        for error in [
            FfError::Load("missing libavcodec.so.62".into()),
            FfError::Abi(AbiError::Version {
                lib: "avcodec",
                found: 61 << 16,
            }),
            FfError::Unsupported("VA-API requires a render node".into()),
        ] {
            let diagnostic = error.to_string();
            assert_eq!(backend_error(error), BackendError::Unsupported(diagnostic));
        }
    }

    #[test]
    fn runtime_failures_remain_retryable() {
        let error = FfError::Failed {
            call: "avcodec_send_frame",
            code: crate::ff_model::AVERROR_ENOMEM,
            name: "ENOMEM".into(),
        };
        assert_eq!(
            backend_error(error),
            BackendError::Failed("avcodec_send_frame failed: ENOMEM".into()),
        );
    }
}
