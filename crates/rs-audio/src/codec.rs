//! Opus encoder and decoder wrappers used by the audio pipeline.
//!
//! The rest of the workspace treats this module as the only interface to
//! libopus. Packets are 10 ms stereo frames at 48 kHz, as specified by 03 §8.3.

use opus::{Application, Bitrate, Channels, Decoder, Encoder};

use crate::{AudioError, FRAME_LEN, FRAME_SAMPLES, MAX_PACKET, SAMPLE_RATE};

/// Encoder settings for 48 kHz stereo Opus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncoderConfig {
    /// Target bitrate in bits per second.
    pub bitrate_bps: i32,
    /// Encoder complexity, from 0 to 10.
    pub complexity: i32,
    /// Enable in-band forward error correction.
    pub inband_fec: bool,
    /// Expected packet loss percentage, from 0 to 100.
    pub packet_loss_perc: i32,
    /// Use the low-delay application mode when true.
    pub low_delay: bool,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            bitrate_bps: 96_000,
            complexity: 8,
            inband_fec: true,
            packet_loss_perc: 5,
            low_delay: false,
        }
    }
}

/// A 48 kHz stereo Opus encoder.
#[derive(Debug)]
pub struct AudioEncoder {
    inner: Encoder,
    lookahead: u32,
}

impl AudioEncoder {
    /// Create an encoder with the supplied settings.
    pub fn new(cfg: &EncoderConfig) -> Result<Self, AudioError> {
        let application = if cfg.low_delay {
            Application::LowDelay
        } else {
            Application::Audio
        };
        let mut inner = Encoder::new(SAMPLE_RATE, Channels::Stereo, application).map_err(codec)?;
        inner
            .set_bitrate(Bitrate::Bits(cfg.bitrate_bps))
            .map_err(codec)?;
        inner.set_complexity(cfg.complexity).map_err(codec)?;
        inner.set_vbr(true).map_err(codec)?;
        inner.set_vbr_constraint(true).map_err(codec)?;
        inner.set_inband_fec(cfg.inband_fec).map_err(codec)?;
        inner
            .set_packet_loss_perc(cfg.packet_loss_perc)
            .map_err(codec)?;
        let lookahead = inner.get_lookahead().map_err(codec)?;
        let lookahead =
            u32::try_from(lookahead).map_err(|_| AudioError::Invalid("negative lookahead"))?;
        Ok(Self { inner, lookahead })
    }

    /// Encode one 10 ms interleaved stereo frame.
    ///
    /// `pcm` must contain exactly [`FRAME_LEN`] samples. `out` must have room
    /// for [`MAX_PACKET`] bytes and the encoded packet is at most that large.
    pub fn encode(&mut self, pcm: &[f32], out: &mut [u8]) -> Result<usize, AudioError> {
        if pcm.len() != FRAME_LEN {
            return Err(AudioError::Invalid("frame length"));
        }
        if out.len() < MAX_PACKET {
            return Err(AudioError::Invalid("output buffer"));
        }

        let mut sanitized = [0.0f32; FRAME_LEN];
        for (clean, &sample) in sanitized.iter_mut().zip(pcm) {
            *clean = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }

        self.inner
            .encode_float(&sanitized, &mut out[..MAX_PACKET])
            .map_err(codec)
    }

    /// Reset the encoder to its freshly created state.
    pub fn reset(&mut self) -> Result<(), AudioError> {
        self.inner.reset_state().map_err(codec)
    }

    /// Change the target bitrate.
    pub fn set_bitrate(&mut self, bps: i32) -> Result<(), AudioError> {
        self.inner.set_bitrate(Bitrate::Bits(bps)).map_err(codec)
    }

    /// Number of samples of algorithmic delay added by the encoder.
    pub fn lookahead_samples(&self) -> Result<u32, AudioError> {
        Ok(self.lookahead)
    }
}

/// A 48 kHz stereo Opus decoder.
#[derive(Debug)]
pub struct AudioDecoder {
    inner: Decoder,
}

impl AudioDecoder {
    /// Create a decoder for 48 kHz stereo packets.
    pub fn new() -> Result<Self, AudioError> {
        Decoder::new(SAMPLE_RATE, Channels::Stereo)
            .map(|inner| Self { inner })
            .map_err(codec)
    }

    /// Decode one packet into exactly one 10 ms frame.
    ///
    /// The return value is the number of samples per channel (480). `out` must
    /// have room for [`FRAME_LEN`] interleaved samples.
    pub fn decode(&mut self, packet: &[u8], out: &mut [f32]) -> Result<usize, AudioError> {
        if packet.is_empty() {
            return Err(AudioError::Invalid("empty packet"));
        }
        validate_output(out)?;

        let samples = self.inner.get_nb_samples(packet).map_err(codec)?;
        if samples != FRAME_SAMPLES {
            return Err(AudioError::Invalid("frame duration"));
        }
        self.decode_into(packet, out, false)
    }

    /// Conceal one lost frame with packet loss concealment.
    pub fn conceal(&mut self, out: &mut [f32]) -> Result<usize, AudioError> {
        validate_output(out)?;
        self.decode_into(&[], out, false)
    }

    /// Recover a lost frame from the next packet's in-band FEC.
    ///
    /// If the next packet has no FEC data, libopus falls back to concealment.
    pub fn recover(&mut self, next_packet: &[u8], out: &mut [f32]) -> Result<usize, AudioError> {
        validate_output(out)?;
        self.decode_into(next_packet, out, true)
    }

    /// Reset the decoder to its freshly created state.
    pub fn reset(&mut self) -> Result<(), AudioError> {
        self.inner.reset_state().map_err(codec)
    }

    fn decode_into(
        &mut self,
        packet: &[u8],
        out: &mut [f32],
        fec: bool,
    ) -> Result<usize, AudioError> {
        let samples = self
            .inner
            .decode_float(packet, &mut out[..FRAME_LEN], fec)
            .map_err(codec)?;
        if samples != FRAME_SAMPLES {
            return Err(AudioError::Invalid("frame duration"));
        }
        Ok(samples)
    }
}

fn validate_output(out: &[f32]) -> Result<(), AudioError> {
    if out.len() < FRAME_LEN {
        return Err(AudioError::Invalid("output buffer"));
    }
    Ok(())
}

fn codec(error: opus::Error) -> AudioError {
    AudioError::Codec {
        function: error.function(),
        code: error.code(),
    }
}
