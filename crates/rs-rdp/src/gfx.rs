//! EGFX graphics pipeline with optional software H.264 decoding.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use ironrdp_connector::{ClientConnector, Config};
use ironrdp_core::{decode_cursor, ReadCursor};
use ironrdp_dvc::{
    encode_dvc_messages, DrdynvcClient, DvcClientProcessor, DvcMessage, DvcProcessor,
};
use ironrdp_egfx::client::{GraphicsPipelineClient, GraphicsPipelineHandler};
use ironrdp_egfx::decode::H264Decoder;
use ironrdp_egfx::pdu::{
    CapabilitiesV103Flags, CapabilitiesV104Flags, CapabilitiesV107Flags, CapabilitiesV10Flags,
    CapabilitiesV81Flags, CapabilitySet, GfxPdu,
};
use ironrdp_graphics::{image_processing::PixelFormat, zgfx};
use ironrdp_pdu::geometry::InclusiveRectangle;
use ironrdp_pdu::{decode_err, PduResult};
use ironrdp_session::image::DecodedImage;
use ironrdp_session::ActiveStage;
use ironrdp_svc::ChannelFlags;

use crate::ext::ExtCtx;
use crate::types::{RdpConfig, RdpError};

/// Before `ClientConnector::new`: connector settings this feature needs.
pub fn configure(cfg: &RdpConfig, _ctx: &ExtCtx, config: &mut Config) {
    if cfg.gfx {
        config.support_dyn_vc_gfx_protocol = true;
    }
}

/// Dynamic virtual channels this feature registers.
pub fn dynamic_channels(cfg: &RdpConfig, ctx: &ExtCtx, drdynvc: DrdynvcClient) -> DrdynvcClient {
    if !cfg.gfx {
        return drdynvc;
    }

    // drain() composites completed frames after ActiveStage processes this DVC.
    let handler = Handler {
        log: tracing::dispatcher::get_default(Clone::clone),
    };
    drdynvc.with_dynamic_channel(PausableGraphics {
        inner: GraphicsPipelineClient::new(Box::new(handler), platform_decoder()),
        background: ctx.background.clone(),
        decompressor: zgfx::Decompressor::new(),
        buffer: Vec::new(),
        full_refresh: false,
    })
}

struct PausableGraphics {
    inner: GraphicsPipelineClient,
    background: Arc<AtomicBool>,
    decompressor: zgfx::Decompressor,
    buffer: Vec<u8>,
    full_refresh: bool,
}

ironrdp_core::impl_as_any!(PausableGraphics);
impl DvcClientProcessor for PausableGraphics {}

impl DvcProcessor for PausableGraphics {
    fn channel_name(&self) -> &str {
        self.inner.channel_name()
    }
    fn start(&mut self, id: u32) -> PduResult<Vec<DvcMessage>> {
        self.inner.start(id)
    }
    fn close(&mut self, id: u32) {
        self.inner.close(id);
    }

    fn process(&mut self, id: u32, payload: &[u8]) -> PduResult<Vec<DvcMessage>> {
        // Keep ZGFX's transport dictionary in sync even when discarding video.
        self.buffer.clear();
        self.buffer.shrink_to(1024 * 1024);
        self.decompressor
            .decompress(payload, &mut self.buffer)
            .map_err(|e| decode_err!(e))?;
        let mut cursor = ReadCursor::new(&self.buffer);
        let mut responses = Vec::new();
        while !cursor.is_empty() {
            let start = cursor.pos();
            let pdu: GfxPdu = decode_cursor(&mut cursor).map_err(|e| decode_err!(e))?;
            if self.background.load(Ordering::Acquire)
                && matches!(
                    pdu,
                    GfxPdu::WireToSurface1(_)
                        | GfxPdu::WireToSurface2(_)
                        | GfxPdu::SolidFill(_)
                        | GfxPdu::SurfaceToSurface(_)
                        | GfxPdu::SurfaceToCache(_)
                        | GfxPdu::CacheToSurface(_)
                )
            {
                continue;
            }
            // Feed uncompressed single-segment ZGFX to the real client. Lifecycle,
            // frame counters and EndFrame ACKs still use IronRDP's state machine.
            let mut uncompressed = vec![0xe0, 0x04];
            uncompressed.extend_from_slice(&self.buffer[start..cursor.pos()]);
            responses.extend(self.inner.process(id, &uncompressed)?);
        }
        Ok(responses)
    }
}

pub(crate) fn refresh(stage: &mut ActiveStage) -> Result<Vec<u8>, RdpError> {
    let Some(mut channel) = stage.get_dvc_mut::<PausableGraphics>() else {
        return Ok(Vec::new());
    };
    let id = channel.channel_id();
    let Some(pdu) = channel.processor_mut().inner.request_protocol_reset() else {
        return Ok(Vec::new());
    };
    let messages = encode_dvc_messages(id, vec![Box::new(pdu)], ChannelFlags::empty())
        .map_err(|e| RdpError::Protocol(e.to_string()))?;
    stage
        .encode_dvc_messages(messages)
        .map_err(|e| RdpError::Protocol(e.to_string()))
}

pub(crate) fn drain(
    stage: &mut ActiveStage,
    image: &mut DecodedImage,
) -> Result<Option<InclusiveRectangle>, RdpError> {
    let Some(mut channel) = stage.get_dvc_mut::<PausableGraphics>() else {
        return Ok(None);
    };
    let graphics = channel.processor_mut();
    if let Some((width, height)) = graphics.inner.take_output_reset() {
        // Pointer rendering is accelerated: its bitmap stays in RdpSink, not image.
        *image = DecodedImage::new(PixelFormat::RgbA32, width, height);
        graphics.full_refresh = true;
    }
    let updates = graphics.inner.drain_output();
    if graphics.background.load(Ordering::Acquire) {
        return Ok(None);
    }
    let full = graphics.full_refresh && !updates.is_empty();
    if full {
        graphics.full_refresh = false;
    }
    let rect = stage
        .composite_rgba32(
            image,
            updates
                .into_iter()
                .map(|update| (update.region, update.data)),
        )
        .map_err(|error| RdpError::Protocol(error.to_string()))?;
    Ok(if full && rect.is_some() {
        Some(InclusiveRectangle {
            left: 0,
            top: 0,
            right: image.width().saturating_sub(1),
            bottom: image.height().saturating_sub(1),
        })
    } else {
        rect
    })
}

/// Static virtual channels this feature registers.
pub fn static_channels(
    cfg: &RdpConfig,
    ctx: &ExtCtx,
    connector: ClientConnector,
) -> ClientConnector {
    let _ = (cfg, ctx);
    connector
}

/// After every `ActiveStage::process` batch and every `ctx.wake`: bytes to send.
pub fn pump(stage: &mut ActiveStage, ctx: &ExtCtx) -> Result<Vec<u8>, RdpError> {
    let _ = (stage, ctx);
    Ok(Vec::new())
}

struct Handler {
    log: tracing::Dispatch,
}

impl GraphicsPipelineHandler for Handler {
    fn on_capabilities_confirmed(&mut self, caps: &CapabilitySet) {
        // client.rs suppresses dependency protocol logs while processing PDUs. Restore
        // the caller's dispatcher only for these metadata-only diagnostics.
        tracing::dispatcher::with_default(&self.log, || {
            tracing::info!(version = ?caps.version(), "RDP EGFX capabilities confirmed");
            let (avc420, avc444) = avc_capabilities(caps);
            tracing::info!(avc420, avc444, "RDP EGFX codecs negotiated");
        });
    }
}

fn avc_capabilities(caps: &CapabilitySet) -> (bool, bool) {
    let enabled = match caps {
        CapabilitySet::V8 { .. } => return (false, false),
        CapabilitySet::V8_1 { flags } => {
            return (flags.contains(CapabilitiesV81Flags::AVC420_ENABLED), false);
        }
        CapabilitySet::V10_1 => true,
        CapabilitySet::V10 { flags } | CapabilitySet::V10_2 { flags } => {
            !flags.contains(CapabilitiesV10Flags::AVC_DISABLED)
        }
        CapabilitySet::V10_3 { flags } => !flags.contains(CapabilitiesV103Flags::AVC_DISABLED),
        CapabilitySet::V10_4 { flags }
        | CapabilitySet::V10_5 { flags }
        | CapabilitySet::V10_6 { flags }
        | CapabilitySet::V10_6Err { flags } => !flags.contains(CapabilitiesV104Flags::AVC_DISABLED),
        CapabilitySet::V10_7 { flags } => !flags.contains(CapabilitiesV107Flags::AVC_DISABLED),
    };
    (enabled, enabled)
}

#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos"))]
fn platform_decoder() -> Option<Box<dyn H264Decoder>> {
    background::start(|| {
        ironrdp_egfx::decode::OpenH264Decoder::new()
            .map(|decoder| Box::new(decoder) as Box<dyn H264Decoder>)
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
fn platform_decoder() -> Option<Box<dyn H264Decoder>> {
    tracing::info!(
        ready = false,
        "RDP H.264 decoder unavailable on this target"
    );
    None
}

// Like IronRDP's official client, start the decoder during connection setup and
// finish starting only when the server opens EGFX. A failed start makes IronRDP
// discard the decoder before advertising capabilities, preserving bitmap support.
#[cfg(any(target_os = "android", target_os = "ios", target_os = "macos", test))]
mod background {
    use std::sync::mpsc;
    use std::time::Duration;

    use ironrdp_egfx::decode::{DecodedFrame, DecoderError, DecoderResult, Yuv420Frame};

    use super::H264Decoder;

    const START_TIMEOUT: Duration = Duration::from_secs(15);

    pub(super) fn start(
        create: impl FnOnce() -> DecoderResult<Box<dyn H264Decoder>> + Send + 'static,
    ) -> Option<Box<dyn H264Decoder>> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let log = tracing::dispatcher::get_default(Clone::clone);
        if let Err(error) = std::thread::Builder::new()
            .name("rdp-h264-start".into())
            .spawn(move || {
                let _ = sender.send(create());
            })
        {
            tracing::warn!(%error, "Unable to start the RDP H.264 decoder worker");
            tracing::info!(ready = false, "RDP H.264 decoder readiness");
            return None;
        }
        Some(Box::new(BackgroundDecoder {
            starting: Some(receiver),
            decoder: None,
            log,
        }))
    }

    struct BackgroundDecoder {
        starting: Option<mpsc::Receiver<DecoderResult<Box<dyn H264Decoder>>>>,
        decoder: Option<Box<dyn H264Decoder>>,
        log: tracing::Dispatch,
    }

    impl BackgroundDecoder {
        fn ready(&mut self) -> DecoderResult<&mut Box<dyn H264Decoder>> {
            self.decoder
                .as_mut()
                .ok_or_else(|| DecoderError::msg("RDP H.264 decoder is not ready"))
        }
    }

    impl H264Decoder for BackgroundDecoder {
        fn decode(&mut self, data: &[u8]) -> DecoderResult<DecodedFrame> {
            self.ready()?.decode(data)
        }

        fn decode_yuv420(&mut self, data: &[u8]) -> DecoderResult<Yuv420Frame> {
            self.ready()?.decode_yuv420(data)
        }

        fn supports_yuv420(&self) -> bool {
            self.decoder
                .as_ref()
                .is_some_and(|decoder| decoder.supports_yuv420())
        }

        fn wait_until_ready(&mut self) -> bool {
            if let Some(receiver) = self.starting.take() {
                tracing::dispatcher::with_default(&self.log, || {
                    match receiver.recv_timeout(START_TIMEOUT) {
                        Ok(Ok(mut decoder)) => {
                            if decoder.wait_until_ready() {
                                self.decoder = Some(decoder);
                            } else {
                                tracing::warn!("RDP H.264 decoder could not become ready");
                            }
                        }
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "Unable to create the RDP H.264 decoder");
                        }
                        Err(error) => {
                            tracing::warn!(%error, "RDP H.264 decoder startup did not complete");
                        }
                    }
                    tracing::info!(
                        ready = self.decoder.is_some(),
                        "RDP H.264 decoder readiness"
                    );
                });
            }
            self.decoder.is_some()
        }

        fn reset(&mut self) {
            if let Some(decoder) = self.decoder.as_mut() {
                decoder.reset();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use ironrdp_core::{decode, encode_vec};
    use ironrdp_dvc::DvcProcessor as _;
    use ironrdp_egfx::decode::{DecodedFrame, DecoderError, DecoderResult, Yuv420Frame};
    use ironrdp_egfx::pdu::GfxPdu;

    use super::*;

    struct CountingDecoder(Arc<std::sync::atomic::AtomicUsize>);

    impl H264Decoder for CountingDecoder {
        fn decode(&mut self, _: &[u8]) -> DecoderResult<DecodedFrame> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(DecodedFrame::new(vec![1, 2, 3, 255], 1, 1))
        }
    }

    fn feed(client: &mut PausableGraphics, pdus: &[GfxPdu]) -> Vec<GfxPdu> {
        let mut bytes = vec![0xe0, 0x04];
        for pdu in pdus {
            bytes.extend(encode_vec(pdu).unwrap());
        }
        client
            .process(1, &bytes)
            .unwrap()
            .iter()
            .map(|pdu| decode(&encode_vec(pdu.as_ref()).unwrap()).unwrap())
            .collect()
    }

    #[test]
    fn paused_gfx_keeps_frame_acks_without_invoking_h264_decoder() {
        use ironrdp_egfx::pdu::{
            encode_avc420_bitmap_stream, CapabilitiesConfirmPdu, Codec1Type, CreateSurfacePdu,
            EndFramePdu, PixelFormat, ResetGraphicsPdu, WireToSurface1Pdu,
        };
        use ironrdp_pdu::geometry::ExclusiveRectangle;
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let background = Arc::new(AtomicBool::new(false));
        let mut client = PausableGraphics {
            inner: GraphicsPipelineClient::new(
                Box::new(Handler {
                    log: tracing::dispatcher::get_default(Clone::clone),
                }),
                Some(Box::new(CountingDecoder(calls.clone()))),
            ),
            background: background.clone(),
            decompressor: zgfx::Decompressor::new(),
            buffer: Vec::new(),
            full_refresh: false,
        };
        client.start(1).unwrap();
        let setup = [
            GfxPdu::CapabilitiesConfirm(CapabilitiesConfirmPdu::from_typed(
                &CapabilitySet::V10_7 {
                    flags: CapabilitiesV107Flags::SMALL_CACHE,
                },
            )),
            GfxPdu::ResetGraphics(ResetGraphicsPdu {
                width: 1,
                height: 1,
                monitors: vec![],
            }),
            GfxPdu::CreateSurface(CreateSurfacePdu {
                surface_id: 1,
                width: 1,
                height: 1,
                pixel_format: PixelFormat::XRgb,
            }),
        ];
        feed(&mut client, &setup);
        let pixels = GfxPdu::WireToSurface1(WireToSurface1Pdu {
            surface_id: 1,
            codec_id: Codec1Type::Avc420,
            pixel_format: PixelFormat::XRgb,
            destination_rectangle: ExclusiveRectangle {
                left: 0,
                top: 0,
                right: 1,
                bottom: 1,
            },
            bitmap_data: encode_avc420_bitmap_stream(&[], b"counted by test decoder"),
        });
        feed(
            &mut client,
            &[
                pixels.clone(),
                GfxPdu::EndFrame(EndFramePdu { frame_id: 1 }),
            ],
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        background.store(true, Ordering::Release);
        let responses = feed(
            &mut client,
            &[
                pixels.clone(),
                GfxPdu::EndFrame(EndFramePdu { frame_id: 2 }),
            ],
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "hidden AVC must not reach the decoder"
        );
        assert!(
            matches!(&responses[0], GfxPdu::FrameAcknowledge(ack) if ack.frame_id == 2 && ack.total_frames_decoded == 2)
        );
        background.store(false, Ordering::Release);
        assert!(client.inner.request_protocol_reset().is_some());
        feed(&mut client, &setup);
        let responses = feed(
            &mut client,
            &[pixels, GfxPdu::EndFrame(EndFramePdu { frame_id: 3 })],
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(
            matches!(&responses[0], GfxPdu::FrameAcknowledge(ack) if ack.frame_id == 3 && ack.total_frames_decoded == 1)
        );
    }

    struct TestDecoder {
        yuv: bool,
    }

    impl H264Decoder for TestDecoder {
        fn decode(&mut self, _: &[u8]) -> DecoderResult<DecodedFrame> {
            Ok(DecodedFrame::new(vec![1, 2, 3, 255], 1, 1))
        }

        fn supports_yuv420(&self) -> bool {
            self.yuv
        }

        fn decode_yuv420(&mut self, _: &[u8]) -> DecoderResult<Yuv420Frame> {
            assert!(self.yuv);
            Yuv420Frame::new(1, 1, vec![64], 1, vec![128], vec![128], 1)
        }
    }

    fn advertised(decoder: Option<Box<dyn H264Decoder>>) -> Vec<CapabilitySet> {
        let handler = Handler {
            log: tracing::dispatcher::get_default(Clone::clone),
        };
        let mut client = GraphicsPipelineClient::new(Box::new(handler), decoder);
        let output = client.start(1).unwrap();
        let bytes = encode_vec(output[0].as_ref()).unwrap();
        let GfxPdu::CapabilitiesAdvertise(caps) = decode(&bytes).unwrap() else {
            panic!("EGFX must start with a capability advertisement");
        };
        caps.0
            .iter()
            .map(|cap| cap.parsed().unwrap().unwrap())
            .collect()
    }

    #[test]
    fn failed_decoder_still_negotiates_without_avc() {
        let decoder = background::start(|| Err(DecoderError::msg("test decoder unavailable")));
        let caps = advertised(decoder);
        assert!(!caps.is_empty());
        assert!(caps
            .iter()
            .all(|cap| avc_capabilities(cap) == (false, false)));
    }

    #[test]
    fn avc444_requires_yuv_planes() {
        for yuv in [false, true] {
            let decoder = background::start(move || Ok(Box::new(TestDecoder { yuv })));
            let caps = advertised(decoder);
            assert!(caps.iter().any(|cap| avc_capabilities(cap).0));
            assert_eq!(caps.iter().any(|cap| avc_capabilities(cap).1), yuv);
        }
    }

    #[test]
    fn decoder_starts_in_background_and_forwards_both_output_formats() {
        let (release, blocked) = mpsc::sync_channel(1);
        let mut decoder = background::start(move || {
            blocked.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(Box::new(TestDecoder { yuv: true }))
        })
        .unwrap();
        // The factory cannot finish until start() has returned to this caller.
        assert!(!decoder.supports_yuv420());
        assert!(decoder.decode(&[]).is_err());
        release.send(()).unwrap();
        assert!(decoder.wait_until_ready());
        assert!(decoder.wait_until_ready(), "readiness is idempotent");
        assert!(decoder.supports_yuv420());
        assert_eq!(decoder.decode(&[]).unwrap().data(), &[1, 2, 3, 255]);
        assert_eq!(
            decoder.decode_yuv420(&[]).unwrap().luma_row(0),
            Some(&[64][..])
        );
    }
}
