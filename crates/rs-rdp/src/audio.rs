//! rdpsnd remote PCM playback over the static and dynamic audio channels.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use ironrdp_connector::{ClientConnector, Config};
use ironrdp_dvc::DrdynvcClient;
use ironrdp_rdpsnd::client::{
    Clock, Rdpsnd, RdpsndClientHandler, RdpsndDvcState, DVC_CHANNEL_NAME, LOSSY_DVC_CHANNEL_NAME,
};
use ironrdp_rdpsnd::pdu::{AudioFormat, PitchPdu, VolumePdu, WaveFormat};
use ironrdp_session::ActiveStage;
use tracing::{debug, info};

use crate::ext::ExtCtx;
use crate::types::{RdpAudioSink, RdpConfig, RdpError};

const fn pcm_format(rate: u32) -> AudioFormat {
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: 2,
        n_samples_per_sec: rate,
        n_avg_bytes_per_sec: rate * 4,
        n_block_align: 4,
        bits_per_sample: 16,
        data: None,
    }
}

static FORMATS: [AudioFormat; 2] = [pcm_format(48_000), pcm_format(44_100)];

struct PcmPlayback {
    sink: Arc<dyn RdpAudioSink>,
    rate: Option<u32>,
    samples: Vec<i16>,
}

// Neither the sink nor the decoded audio belongs in protocol diagnostics.
impl fmt::Debug for PcmPlayback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PcmPlayback")
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

impl PcmPlayback {
    fn new(sink: &Arc<dyn RdpAudioSink>) -> Self {
        Self {
            sink: Arc::clone(sink),
            rate: None,
            samples: Vec::new(),
        }
    }
}

impl RdpsndClientHandler for PcmPlayback {
    fn get_formats(&self) -> &[AudioFormat] {
        &FORMATS
    }

    fn wave(&mut self, format: &AudioFormat, _ts: u32, data: Cow<'_, [u8]>) {
        // IronRDP resolves the format number against our advertised list. A
        // truncated stereo frame must not shift the channels of the next block.
        if !data.len().is_multiple_of(4) {
            debug!(bytes = data.len(), "Ignoring incomplete rdpsnd PCM frame");
            return;
        }
        if self.rate != Some(format.n_samples_per_sec) {
            self.rate = Some(format.n_samples_per_sec);
            info!(
                sample_rate = format.n_samples_per_sec,
                channels = format.n_channels,
                bits_per_sample = format.bits_per_sample,
                "Negotiated rdpsnd PCM playback"
            );
            self.sink
                .format(format.n_samples_per_sec, format.n_channels);
        }
        self.samples.clear();
        self.samples.extend(
            data.chunks_exact(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]])),
        );
        // These callbacks run inside ActiveStage::process. The sink must enqueue
        // or copy promptly; there is no playback wait, resampling, or worker here.
        self.sink.samples(&self.samples);
    }

    fn set_volume(&mut self, _volume: VolumePdu) {
        debug!("Received rdpsnd volume change; playback uses the system volume");
    }

    fn set_pitch(&mut self, _pitch: PitchPdu) {
        debug!("Received rdpsnd pitch change");
    }

    fn close(&mut self) {
        self.rate = None;
        self.samples.clear();
        self.sink.stop();
    }
}

fn clock() -> Clock {
    let origin = Instant::now();
    Box::new(move || origin.elapsed())
}

/// Before `ClientConnector::new`: connector settings this feature needs.
pub fn configure(_cfg: &RdpConfig, ctx: &ExtCtx, config: &mut Config) {
    config.enable_audio_playback = ctx.audio.is_some();
}

/// Dynamic virtual channels this feature registers.
pub fn dynamic_channels(_cfg: &RdpConfig, ctx: &ExtCtx, drdynvc: DrdynvcClient) -> DrdynvcClient {
    let Some(sink) = &ctx.audio else {
        return drdynvc;
    };
    // The lossy channel indexes formats negotiated on the reliable DVC. Keep
    // that state across channel reopens, as in IronRDP's official client.
    let state = RdpsndDvcState::new(Box::new(PcmPlayback::new(sink))).with_clock(clock());
    let lossy = state.clone();
    drdynvc
        .with_dynamic_channel_factory(DVC_CHANNEL_NAME, move || state.channel())
        .with_dynamic_channel_factory(LOSSY_DVC_CHANNEL_NAME, move || lossy.lossy_channel())
}

/// Static virtual channels this feature registers.
pub fn static_channels(
    _cfg: &RdpConfig,
    ctx: &ExtCtx,
    connector: ClientConnector,
) -> ClientConnector {
    let Some(sink) = &ctx.audio else {
        return connector;
    };
    connector.with_static_channel(Rdpsnd::new(Box::new(PcmPlayback::new(sink))).with_clock(clock()))
}

/// After every `ActiveStage::process` batch and every `ctx.wake`: bytes to send.
pub fn pump(stage: &mut ActiveStage, ctx: &ExtCtx) -> Result<Vec<u8>, RdpError> {
    let _ = (stage, ctx);
    Ok(Vec::new())
}
