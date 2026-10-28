//! Display suppression without stopping the RDP transport or virtual channels.

use ironrdp_core::{decode_cursor, encode_vec, Encode, ReadCursor, WriteBuf};
use ironrdp_pdu::codecs::rfx::FrameAcknowledgePdu;
use ironrdp_pdu::fast_path::{
    FastPathHeader, FastPathUpdate, FastPathUpdatePdu, Fragmentation, UpdateCode,
};
use ironrdp_pdu::geometry::InclusiveRectangle;
use ironrdp_pdu::mcs::{self, SendDataIndication, SendDataIndicationCtx};
use ironrdp_pdu::rdp::headers::{decode_io_channel, IoChannelPdu, ShareDataPdu};
use ironrdp_pdu::rdp::refresh_rectangle::RefreshRectanglePdu;
use ironrdp_pdu::rdp::suppress_output::SuppressOutputPdu;
use ironrdp_pdu::surface_commands::{FrameAction, SurfaceCommand};
use ironrdp_pdu::x224::X224;
use ironrdp_pdu::Action;
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageOutput};

use crate::{gfx, RdpError};

pub(crate) struct OutputControl {
    pub paused: bool,
    pub suppress: bool,
    pub refresh: bool,
    pub io_channel: u16,
    applied: Option<bool>,
    // Keep surface-command framing across visibility changes so an in-flight
    // RemoteFX frame still gets its acknowledgement, without decoding its pixels.
    surface_fragment: Option<Vec<u8>>,
    discard_fragment: bool,
}

fn protocol(error: impl std::fmt::Display) -> RdpError {
    RdpError::Protocol(error.to_string())
}

impl OutputControl {
    pub fn new(suppress: bool, refresh: bool, io_channel: u16) -> Self {
        Self {
            paused: false,
            suppress,
            refresh,
            io_channel,
            applied: Some(false),
            surface_fragment: None,
            discard_fragment: false,
        }
    }

    pub fn method(&self) -> &'static str {
        if self.suppress {
            "suppress-output"
        } else {
            "skip-decode"
        }
    }

    pub fn reactivated(&mut self, suppress: bool, refresh: bool, io_channel: u16) {
        self.suppress = suppress;
        self.refresh = refresh;
        self.io_channel = io_channel;
        self.applied = None;
        self.surface_fragment = None;
        self.discard_fragment = false;
    }

    pub fn apply(
        &mut self,
        stage: &mut ActiveStage,
        width: u16,
        height: u16,
    ) -> Result<Vec<u8>, RdpError> {
        if self.applied == Some(self.paused) {
            return Ok(Vec::new());
        }
        let rect = InclusiveRectangle {
            left: 0,
            top: 0,
            right: width.saturating_sub(1),
            bottom: height.saturating_sub(1),
        };
        let mut bytes = WriteBuf::new();
        if self.suppress {
            stage
                .encode_static(
                    &mut bytes,
                    ShareDataPdu::SuppressOutput(SuppressOutputPdu {
                        desktop_rect: (!self.paused).then_some(rect.clone()),
                    }),
                )
                .map_err(protocol)?;
        }
        if self.paused {
            self.discard_fragment = true;
        } else {
            // Restart codec/cache state on EGFX 10.3+ before requesting pixels.
            // Older peers use the standard Suppress Output / Refresh Rect redraw.
            let reset = gfx::refresh(stage)?;
            bytes.write_slice(&reset);
            if self.refresh {
                stage
                    .encode_static(
                        &mut bytes,
                        ShareDataPdu::RefreshRectangle(RefreshRectanglePdu {
                            areas_to_refresh: vec![rect],
                        }),
                    )
                    .map_err(protocol)?;
            }
        }
        self.applied = Some(self.paused);
        Ok(bytes.into_inner())
    }

    pub fn process(
        &mut self,
        stage: &mut ActiveStage,
        image: &mut DecodedImage,
        action: Action,
        frame: &[u8],
        received_at: Option<ironrdp_core::MonotonicInstant>,
    ) -> Result<Vec<ActiveStageOutput>, RdpError> {
        let mut outputs = match action {
            Action::FastPath => self.fast_path(stage, image, frame, received_at)?,
            Action::X224 if self.paused => self.paused_x224(stage, image, frame, received_at)?,
            _ => stage
                .process(image, action, frame, received_at)
                .map_err(protocol)?,
        };
        if let Some(rect) = gfx::drain(stage, image)? {
            outputs.push(ActiveStageOutput::GraphicsUpdate(rect));
        }
        Ok(outputs)
    }

    fn fast_path(
        &mut self,
        stage: &mut ActiveStage,
        image: &mut DecodedImage,
        frame: &[u8],
        received_at: Option<ironrdp_core::MonotonicInstant>,
    ) -> Result<Vec<ActiveStageOutput>, RdpError> {
        let mut cursor = ReadCursor::new(frame);
        let header: FastPathHeader = decode_cursor(&mut cursor).map_err(protocol)?;
        let mut updates = Vec::new();
        while !cursor.is_empty() {
            match decode_cursor::<FastPathUpdatePdu<'_>>(&mut cursor) {
                Ok(update) => updates.push(update),
                Err(_) if !self.paused => {
                    // Preserve IronRDP's recovery of malformed foreground bitmaps.
                    return stage
                        .process(image, Action::FastPath, frame, received_at)
                        .map_err(protocol);
                }
                Err(error) => return Err(protocol(error)),
            }
        }
        let mut outputs = Vec::new();
        for update in updates {
            // connector_config deliberately does not negotiate bulk compression;
            // ZGFX has its own history, maintained by the graphics-channel wrapper.
            if update
                .compression_flags
                .is_some_and(|flags| !flags.is_empty())
            {
                return Err(protocol("unnegotiated fast-path bulk compression"));
            }
            if matches!(
                update.fragmentation,
                Fragmentation::Single | Fragmentation::First
            ) {
                self.discard_fragment = self.paused;
                self.surface_fragment = None;
            }
            // Cursor and palette caches stay valid across a pause; dispatch_outputs
            // suppresses their presentation. Only desktop pixel decoding is skipped.
            let pixels = matches!(
                update.update_code,
                UpdateCode::Bitmap | UpdateCode::SurfaceCommands | UpdateCode::Orders
            );
            let discard = pixels && (self.paused || self.discard_fragment);
            let surface = update.update_code == UpdateCode::SurfaceCommands;
            let complete = match update.fragmentation {
                Fragmentation::Single if surface && discard => Some(update.data.to_vec()),
                Fragmentation::First if surface => {
                    self.surface_fragment = Some(update.data.to_vec());
                    None
                }
                Fragmentation::Next | Fragmentation::Last if surface => {
                    if let Some(data) = &mut self.surface_fragment {
                        if data.len().saturating_add(update.data.len()) > 64 * 1024 * 1024 {
                            return Err(protocol("surface update too large"));
                        }
                        data.extend_from_slice(update.data);
                    }
                    if update.fragmentation == Fragmentation::Last {
                        self.surface_fragment.take()
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if discard {
                if let Some(data) = complete {
                    let FastPathUpdate::SurfaceCommands(commands) =
                        FastPathUpdate::decode_with_code(&data, UpdateCode::SurfaceCommands)
                            .map_err(protocol)?
                    else {
                        unreachable!();
                    };
                    let mut ack = WriteBuf::new();
                    for command in commands {
                        if let SurfaceCommand::FrameMarker(marker) = command {
                            if marker.frame_action == FrameAction::End {
                                stage
                                    .encode_static(
                                        &mut ack,
                                        ShareDataPdu::FrameAcknowledge(FrameAcknowledgePdu {
                                            frame_id: marker.frame_id.unwrap_or(0),
                                        }),
                                    )
                                    .map_err(protocol)?;
                            }
                        }
                    }
                    if !ack.filled().is_empty() {
                        outputs.push(ActiveStageOutput::ResponseFrame(ack.into_inner()));
                    }
                }
            } else {
                let mut bytes = encode_vec(&FastPathHeader::new(header.flags, update.size()))
                    .map_err(protocol)?;
                bytes.extend_from_slice(&encode_vec(&update).map_err(protocol)?);
                outputs.extend(
                    stage
                        .process(image, Action::FastPath, &bytes, received_at)
                        .map_err(protocol)?,
                );
            }
        }
        Ok(outputs)
    }

    fn paused_x224(
        &self,
        stage: &mut ActiveStage,
        image: &mut DecodedImage,
        frame: &[u8],
        received_at: Option<ironrdp_core::MonotonicInstant>,
    ) -> Result<Vec<ActiveStageOutput>, RdpError> {
        let Ok(data) = mcs::decode_send_data_indication(frame) else {
            return stage
                .process(image, Action::X224, frame, received_at)
                .map_err(protocol);
        };
        if data.channel_id != self.io_channel {
            return stage
                .process(image, Action::X224, frame, received_at)
                .map_err(protocol);
        }
        let mut remaining = data.user_data;
        let mut outputs = Vec::new();
        while !remaining.is_empty() {
            let len = remaining
                .get(..2)
                .map(|bytes| usize::from(u16::from_le_bytes([bytes[0], bytes[1]])))
                .unwrap_or(0);
            if len == 0 || len > remaining.len() {
                outputs.extend(
                    stage
                        .process(image, Action::X224, frame, received_at)
                        .map_err(protocol)?,
                );
                break;
            }
            let (part, rest) = remaining.split_at(len);
            remaining = rest;
            let pdu = decode_io_channel(SendDataIndicationCtx {
                user_data: part,
                ..data
            })
            .map_err(protocol)?;
            if matches!(pdu, IoChannelPdu::Data(ref ctx) if matches!(ctx.pdu, ShareDataPdu::Update(_)))
            {
                continue;
            }
            let bytes = encode_vec(&X224(SendDataIndication {
                initiator_id: data.initiator_id,
                channel_id: data.channel_id,
                user_data: part.into(),
            }))
            .map_err(protocol)?;
            outputs.extend(
                stage
                    .process(image, Action::X224, &bytes, received_at)
                    .map_err(protocol)?,
            );
        }
        Ok(outputs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_core::decode;
    use ironrdp_graphics::image_processing::PixelFormat;
    use ironrdp_pdu::fast_path::EncryptionFlags;
    use ironrdp_pdu::geometry::ExclusiveRectangle;
    use ironrdp_pdu::mcs::SendDataRequest;
    use ironrdp_pdu::rdp::headers::{ShareControlHeader, ShareControlPdu};
    use ironrdp_pdu::surface_commands::{ExtendedBitmapDataPdu, FrameMarkerPdu, SurfaceBitsPdu};
    use ironrdp_session::ActiveStageBuilder;

    fn stage() -> ActiveStage {
        ActiveStageBuilder {
            static_channels: Default::default(),
            user_channel_id: 1001,
            io_channel_id: 1003,
            message_channel_id: None,
            share_id: 7,
            compression_type: None,
            enable_server_pointer: true,
            pointer_software_rendering: false,
        }
        .build()
    }

    fn pdus(mut bytes: &[u8]) -> Vec<ShareDataPdu> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            let len = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
            let X224(request): X224<SendDataRequest<'_>> = decode(&bytes[..len]).unwrap();
            assert_eq!((request.initiator_id, request.channel_id), (1001, 1003));
            let header: ShareControlHeader = decode(&request.user_data).unwrap();
            assert_eq!(header.share_id, 7);
            let ShareControlPdu::Data(data) = header.share_control_pdu else {
                panic!("expected share data")
            };
            out.push(data.share_data_pdu);
            bytes = &bytes[len..];
        }
        out
    }

    fn fast(data: &[u8], code: UpdateCode, fragmentation: Fragmentation) -> Vec<u8> {
        let pdu = FastPathUpdatePdu {
            fragmentation,
            update_code: code,
            compression_flags: None,
            compression_type: None,
            data,
        };
        let mut bytes =
            encode_vec(&FastPathHeader::new(EncryptionFlags::empty(), pdu.size())).unwrap();
        bytes.extend(encode_vec(&pdu).unwrap());
        bytes
    }

    #[test]
    fn negotiated_suppression_and_full_refresh_are_idempotent() {
        for (suppress, refresh) in [(true, true), (true, false), (false, true), (false, false)] {
            let mut stage = stage();
            let mut output = OutputControl::new(suppress, refresh, 1003);
            assert!(output.apply(&mut stage, 1024, 768).unwrap().is_empty());
            output.paused = true;
            let pause = pdus(&output.apply(&mut stage, 1024, 768).unwrap());
            assert_eq!(pause.len(), usize::from(suppress));
            if suppress {
                assert!(
                    matches!(&pause[0], ShareDataPdu::SuppressOutput(pdu) if pdu.desktop_rect.is_none())
                );
            }
            assert!(output.apply(&mut stage, 1024, 768).unwrap().is_empty());
            output.paused = false;
            let resume = pdus(&output.apply(&mut stage, 1024, 768).unwrap());
            let rect = InclusiveRectangle {
                left: 0,
                top: 0,
                right: 1023,
                bottom: 767,
            };
            assert_eq!(resume.len(), usize::from(suppress) + usize::from(refresh));
            if suppress {
                assert!(
                    matches!(&resume[0], ShareDataPdu::SuppressOutput(pdu) if pdu.desktop_rect.as_ref() == Some(&rect))
                );
            }
            if refresh {
                assert!(
                    matches!(resume.last().unwrap(), ShareDataPdu::RefreshRectangle(pdu) if pdu.areas_to_refresh == [rect])
                );
            }
            assert!(output.apply(&mut stage, 1024, 768).unwrap().is_empty());
            output.paused = true;
            output.reactivated(true, true, 1003);
            assert!(
                matches!(&pdus(&output.apply(&mut stage, 640, 480).unwrap())[0], ShareDataPdu::SuppressOutput(pdu) if pdu.desktop_rect.is_none())
            );
        }
    }

    #[test]
    fn paused_remotefx_skips_invalid_pixels_but_acknowledges_fragmented_frames() {
        let mut stage = stage();
        let mut output = OutputControl::new(false, true, 1003);
        let mut image = DecodedImage::new(PixelFormat::RgbA32, 1, 1);
        let before = image.data().to_vec();
        let commands = FastPathUpdate::SurfaceCommands(vec![
            SurfaceCommand::SetSurfaceBits(SurfaceBitsPdu {
                destination: ExclusiveRectangle {
                    left: 0,
                    top: 0,
                    right: 1,
                    bottom: 1,
                },
                extended_bitmap_data: ExtendedBitmapDataPdu {
                    bpp: 32,
                    codec_id: 3,
                    width: 1,
                    height: 1,
                    header: None,
                    data: b"not remotefx",
                },
            }),
            SurfaceCommand::FrameMarker(FrameMarkerPdu {
                frame_action: FrameAction::End,
                frame_id: Some(42),
            }),
        ]);
        let data = encode_vec(&commands).unwrap();
        let first = fast(
            &data[..10],
            UpdateCode::SurfaceCommands,
            Fragmentation::First,
        );
        // Frame starts in foreground, then finishes after pause.
        output
            .process(&mut stage, &mut image, Action::FastPath, &first, None)
            .unwrap();
        output.paused = true;
        output.apply(&mut stage, 1, 1).unwrap();
        let last = fast(
            &data[10..],
            UpdateCode::SurfaceCommands,
            Fragmentation::Last,
        );
        let out = output
            .process(&mut stage, &mut image, Action::FastPath, &last, None)
            .unwrap();
        let [ActiveStageOutput::ResponseFrame(bytes)] = out.as_slice() else {
            panic!("expected frame ACK")
        };
        assert!(
            matches!(&pdus(bytes)[0], ShareDataPdu::FrameAcknowledge(ack) if ack.frame_id == 42)
        );
        assert_eq!(image.data(), before);
        // The same complete payload would reach the real decoder in foreground.
        let mut foreground = OutputControl::new(false, true, 1003);
        assert!(foreground
            .process(
                &mut stage,
                &mut image,
                Action::FastPath,
                &fast(&data, UpdateCode::SurfaceCommands, Fragmentation::Single),
                None
            )
            .is_err());
    }

    #[test]
    fn paused_slow_path_skips_pixels_and_preserves_session_control() {
        use ironrdp_pdu::rdp::client_info::CompressionType;
        use ironrdp_pdu::rdp::headers::{CompressionFlags, ShareDataHeader, StreamPriority};
        use ironrdp_pdu::rdp::server_error_info::{
            ErrorInfo, ProtocolIndependentCode, ServerSetErrorInfoPdu,
        };
        let mut stage = stage();
        let mut image = DecodedImage::new(PixelFormat::RgbA32, 1, 1);
        let before = image.data().to_vec();
        let mut output = OutputControl::new(false, true, 1003);
        output.paused = true;
        let control = encode_vec(&ShareControlHeader {
            pdu_source: 1001,
            share_id: 7,
            share_control_pdu: ShareControlPdu::Data(ShareDataHeader {
                share_data_pdu: ShareDataPdu::ServerSetErrorInfo(ServerSetErrorInfoPdu(
                    ErrorInfo::ProtocolIndependentCode(
                        ProtocolIndependentCode::RpcInitiatedDisconnect,
                    ),
                )),
                stream_priority: StreamPriority::Medium,
                compression_flags: CompressionFlags::empty(),
                compression_type: CompressionType::K8,
            }),
        })
        .unwrap();
        // IronRDP decodes but cannot encode ShareDataPdu::Update. Reuse the
        // four-byte payload's wire header, setting TS_SHAREDATAHEADER.pduType2
        // to PDUTYPE2_UPDATE (offset 14), followed by invalid bitmap data.
        let mut data = control.clone();
        data[14] = 0x02;
        data[18..].fill(0xff);
        data.extend(control);
        let wire = encode_vec(&X224(SendDataIndication {
            initiator_id: 1001,
            channel_id: 1003,
            user_data: data.into(),
        }))
        .unwrap();
        let out = output
            .process(&mut stage, &mut image, Action::X224, &wire, None)
            .unwrap();
        assert!(matches!(out.as_slice(), [ActiveStageOutput::Terminate(_)]));
        assert_eq!(image.data(), before);
    }

    #[test]
    fn resume_discards_remainder_of_a_paused_fragment_then_decodes_new_updates() {
        let mut stage = stage();
        let mut output = OutputControl::new(false, true, 1003);
        let mut image = DecodedImage::new(PixelFormat::RgbA32, 1, 1);
        output.paused = true;
        output.apply(&mut stage, 1, 1).unwrap();
        output
            .process(
                &mut stage,
                &mut image,
                Action::FastPath,
                &fast(b"bad", UpdateCode::Bitmap, Fragmentation::First),
                None,
            )
            .unwrap();
        output.paused = false;
        output.apply(&mut stage, 1, 1).unwrap();
        assert!(output
            .process(
                &mut stage,
                &mut image,
                Action::FastPath,
                &fast(b"tail", UpdateCode::Bitmap, Fragmentation::Last),
                None
            )
            .unwrap()
            .is_empty());
        let out = output
            .process(
                &mut stage,
                &mut image,
                Action::FastPath,
                &fast(&[], UpdateCode::DefaultPointer, Fragmentation::Single),
                None,
            )
            .unwrap();
        assert!(out
            .iter()
            .any(|event| matches!(event, ActiveStageOutput::PointerDefault)));
    }
}
