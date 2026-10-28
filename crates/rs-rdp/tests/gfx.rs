#![cfg(feature = "testkit")]

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ironrdp_dvc::encode_dvc_messages;
use ironrdp_egfx::pdu::{CapabilitiesAdvertisePdu, CapabilitySet};
use ironrdp_egfx::server::{GraphicsPipelineHandler, GraphicsPipelineServer};
use ironrdp_server::{
    EgfxServerMessage, GfxDvcBridge, GfxServerFactory, GfxServerHandle, ServerEvent,
    ServerEventSender,
};
use ironrdp_svc::ChannelFlags;
use rs_rdp::client::{connect, RdpConnection};
use rs_rdp::testkit::{FakeRdpServer, FakeSecurity};
use rs_rdp::{CertCheck, RdpCommand, RdpConfig, RdpSink, Rect};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};

const WIDTH: u16 = 320;
const HEIGHT: u16 = 240;
const COLOR: [u8; 4] = [200, 90, 40, 255];

async fn wall_clock(test: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(20), test)
        .await
        .expect("EGFX test exceeded its 20-second wall-clock budget");
}

#[derive(Debug)]
enum GfxEvent {
    Advertised(Vec<CapabilitySet>),
    Ready(CapabilitySet),
    FrameAck(u32),
}

struct ServerHandler(mpsc::UnboundedSender<GfxEvent>);

impl GraphicsPipelineHandler for ServerHandler {
    fn capabilities_advertise(&mut self, pdu: &CapabilitiesAdvertisePdu) {
        let caps = pdu
            .0
            .iter()
            .map(|cap| cap.parsed().unwrap().unwrap())
            .collect();
        let _ = self.0.send(GfxEvent::Advertised(caps));
    }

    fn on_ready(&mut self, negotiated: &CapabilitySet) {
        let _ = self.0.send(GfxEvent::Ready(negotiated.clone()));
    }

    fn on_frame_ack(&mut self, frame_id: u32, _: u32, _: u32) {
        let _ = self.0.send(GfxEvent::FrameAck(frame_id));
    }
}

#[derive(Clone)]
struct GfxProbe {
    server: GfxServerHandle,
    sender: Arc<Mutex<Option<mpsc::UnboundedSender<ServerEvent>>>>,
    events: mpsc::UnboundedSender<GfxEvent>,
}

impl ServerEventSender for GfxProbe {
    fn set_sender(&mut self, sender: mpsc::UnboundedSender<ServerEvent>) {
        *self.sender.lock().unwrap() = Some(sender);
    }
}

impl GfxServerFactory for GfxProbe {
    fn build_gfx_handler(&self) -> Box<dyn GraphicsPipelineHandler> {
        Box::new(ServerHandler(self.events.clone()))
    }

    fn build_server_with_handle(&self) -> Option<(GfxDvcBridge, GfxServerHandle)> {
        Some((GfxDvcBridge::new(self.server.clone()), self.server.clone()))
    }
}

impl GfxProbe {
    fn flush(&self) {
        let messages = {
            let mut server = self.server.lock().unwrap();
            let channel_id = server.channel_id().expect("EGFX channel must be open");
            encode_dvc_messages(
                channel_id,
                server.drain_output(),
                ChannelFlags::SHOW_PROTOCOL,
            )
            .unwrap()
        };
        assert!(!messages.is_empty());
        self.sender
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(ServerEvent::Egfx(EgfxServerMessage::SendMessages {
                messages,
            }))
            .unwrap();
    }

    fn surface(&self, region: Rect) -> u16 {
        let mut server = self.server.lock().unwrap();
        let surface_id = server.create_surface(region.w, region.h).unwrap();
        assert!(server.map_surface_to_output(surface_id, u32::from(region.x), u32::from(region.y)));
        surface_id
    }
}

async fn gfx_server() -> (FakeRdpServer, GfxProbe, mpsc::UnboundedReceiver<GfxEvent>) {
    let (events, receiver) = mpsc::unbounded_channel();
    let mut server = GraphicsPipelineServer::new(Box::new(ServerHandler(events.clone())));
    server.set_output_dimensions(WIDTH, HEIGHT);
    let probe = GfxProbe {
        server: Arc::new(Mutex::new(server)),
        sender: Arc::new(Mutex::new(None)),
        events,
    };
    let factory = probe.clone();
    let server = FakeRdpServer::start_with(FakeSecurity::Tls, WIDTH, HEIGHT, move |builder| {
        builder.with_gfx_factory(Some(Box::new(factory)))
    })
    .await
    .unwrap();
    (server, probe, receiver)
}

#[derive(Clone)]
enum SinkEvent {
    Update(Rect, Vec<u8>),
    Commit,
}

#[derive(Default)]
struct Sink {
    events: Mutex<Vec<SinkEvent>>,
    changed: Notify,
}

impl RdpSink for Sink {
    fn resize(&self, w: u16, h: u16) {
        assert_eq!((w, h), (WIDTH, HEIGHT));
    }

    fn update(&self, image: &[u8], stride: usize, rect: Rect) {
        let mut pixels = Vec::new();
        for row in usize::from(rect.y)..usize::from(rect.y + rect.h) {
            let start = row * stride + usize::from(rect.x) * 4;
            pixels.extend_from_slice(&image[start..start + usize::from(rect.w) * 4]);
        }
        self.events
            .lock()
            .unwrap()
            .push(SinkEvent::Update(rect, pixels));
    }

    fn commit(&self) {
        self.events.lock().unwrap().push(SinkEvent::Commit);
        self.changed.notify_one();
    }

    fn pointer_bitmap(&self, _: u16, _: u16, _: u16, _: u16, _: &[u8]) {}
    fn pointer_hidden(&self) {}
    fn pointer_default(&self) {}
    fn pointer_pos(&self, _: u16, _: u16) {}
}

impl Sink {
    async fn assert_region(&self, expected: Rect, color: [u8; 4], tolerance: u8) {
        let (rect, pixels) = loop {
            let notified = self.changed.notified();
            let events = self.events.lock().unwrap().clone();
            // ResetGraphics can also produce a committed black image. Prefer the
            // latest update and wait for the frame's nonzero pixels before checking
            // every pixel, including the edges, and its following commit.
            let update = events.iter().enumerate().rev().find_map(|(index, event)| {
                let SinkEvent::Update(rect, pixels) = event else {
                    return None;
                };
                let covers = rect.x <= expected.x
                    && rect.y <= expected.y
                    && rect.x + rect.w >= expected.x + expected.w
                    && rect.y + rect.h >= expected.y + expected.h;
                if !covers
                    || !events[index + 1..]
                        .iter()
                        .any(|event| matches!(event, SinkEvent::Commit))
                {
                    return None;
                }
                let start = (usize::from(expected.y - rect.y) * usize::from(rect.w)
                    + usize::from(expected.x - rect.x))
                    * 4;
                pixels[start..start + 3]
                    .iter()
                    .any(|byte| *byte != 0)
                    .then(|| (*rect, pixels.clone()))
            });
            if let Some(update) = update {
                break update;
            }
            notified.await;
        };
        for y in expected.y..expected.y + expected.h {
            for x in expected.x..expected.x + expected.w {
                let offset =
                    (usize::from(y - rect.y) * usize::from(rect.w) + usize::from(x - rect.x)) * 4;
                for (actual, expected) in pixels[offset..offset + 4].iter().zip(color) {
                    assert!(
                        actual.abs_diff(expected) <= tolerance,
                        "pixel ({x}, {y}): {:?}, expected {color:?} ± {tolerance}",
                        &pixels[offset..offset + 4]
                    );
                }
            }
        }
    }
}

struct AcceptCertificate;

#[async_trait]
impl CertCheck for AcceptCertificate {
    async fn check(&self, _: [u8; 32]) -> bool {
        true
    }
}

async fn open(server: &FakeRdpServer, gfx: bool, sink: Arc<Sink>) -> RdpConnection {
    let stream = TcpStream::connect(server.addr()).await.unwrap();
    stream.set_nodelay(true).unwrap();
    connect(
        stream,
        RdpConfig {
            server_name: "127.0.0.1".into(),
            credentials: None,
            width: WIDTH,
            height: HEIGHT,
            scale_percent: 100,
            client_name: "gfx-test".into(),
            cert: Arc::new(AcceptCertificate),
            gfx,
            monitors: 1,
            audio: None,
            clipboard: false,
        },
        sink,
    )
    .await
    .unwrap()
}

async fn session(connection: RdpConnection, action: impl Future<Output = ()>) {
    let (commands, receiver) = mpsc::channel::<RdpCommand>(1);
    let (result, ()) = tokio::join!(connection.run(receiver), async {
        action.await;
        drop(commands);
    });
    assert_eq!(result, Ok(()));
}

async fn negotiate(
    events: &mut mpsc::UnboundedReceiver<GfxEvent>,
) -> (Vec<CapabilitySet>, CapabilitySet) {
    let Some(GfxEvent::Advertised(advertised)) = events.recv().await else {
        panic!("client must advertise EGFX capabilities");
    };
    let Some(GfxEvent::Ready(negotiated)) = events.recv().await else {
        panic!("server must confirm EGFX capabilities");
    };
    (advertised, negotiated)
}

async fn bitmap(server: &FakeRdpServer, sink: &Sink) {
    let region = Rect {
        x: 0,
        y: 0,
        w: 16,
        h: 16,
    };
    // Pure red is exact even through the test server's legacy bitmap codecs.
    server.push_bitmap(0, 0, 16, 16, [0, 0, 255, 255].repeat(16 * 16));
    sink.assert_region(region, [255, 0, 0, 255], 0).await;
}

#[tokio::test]
async fn gfx_disabled_uses_bitmaps() {
    wall_clock(async {
        let (server, probe, mut events) = gfx_server().await;
        let sink = Arc::new(Sink::default());
        let connection = open(&server, false, sink.clone()).await;
        session(connection, async {
            bitmap(&server, &sink).await;
            assert!(
                tokio::time::timeout(Duration::from_millis(250), events.recv())
                    .await
                    .is_err()
            );
            let gfx = probe.server.lock().unwrap();
            assert!(gfx.channel_id().is_none(), "disabled client opened EGFX");
            assert!(!gfx.is_ready());
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn gfx_negotiates() {
    wall_clock(async {
        let (server, probe, mut events) = gfx_server().await;
        let sink = Arc::new(Sink::default());
        let connection = open(&server, true, sink).await;
        session(connection, async {
            let (advertised, negotiated) = negotiate(&mut events).await;
            assert!(advertised.contains(&negotiated));
            let gfx = probe.server.lock().unwrap();
            assert!(gfx.channel_id().is_some());
            assert!(gfx.is_ready());
            #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
            {
                assert!(advertised
                    .iter()
                    .all(|cap| matches!(cap, CapabilitySet::V8 { .. })));
                assert!(!gfx.supports_avc420());
                assert!(!gfx.supports_avc444());
            }
            #[cfg(any(target_os = "android", target_os = "ios", target_os = "macos"))]
            {
                assert!(gfx.supports_avc420());
                assert!(gfx.supports_avc444(), "OpenH264 returns YUV planes");
            }
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn gfx_uncompressed_surface() {
    wall_clock(async {
        let (server, probe, mut events) = gfx_server().await;
        let sink = Arc::new(Sink::default());
        let connection = open(&server, true, sink.clone()).await;
        session(connection, async {
            negotiate(&mut events).await;
            // A nonzero origin checks compositor mapping and full-image sink stride.
            let region = Rect {
                x: 23,
                y: 31,
                w: 16,
                h: 16,
            };
            let surface_id = probe.surface(region);
            let frame_id = probe
                .server
                .lock()
                .unwrap()
                .send_uncompressed_frame(
                    surface_id,
                    &[COLOR[2], COLOR[1], COLOR[0], 255].repeat(16 * 16),
                    16,
                    16,
                    0,
                )
                .unwrap();
            probe.flush();
            sink.assert_region(region, COLOR, 0).await;
            assert!(matches!(events.recv().await, Some(GfxEvent::FrameAck(id)) if id == frame_id));
        })
        .await;
    })
    .await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn gfx_h264_roundtrip() {
    use ironrdp_egfx::pdu::Avc420Region;
    use openh264::encoder::Encoder;
    use openh264::formats::YUVBuffer;

    wall_clock(async {
        // MS-RDPEGFX specifies full-range BT.709; the generic RGB-to-YUV helper
        // uses a different matrix. These planes encode COLOR with the RDP matrix.
        let mut yuv = vec![110; 64 * 64];
        yuv.extend(vec![90; 32 * 32]);
        yuv.extend(vec![185; 32 * 32]);
        let frame = YUVBuffer::from_vec(yuv, 64, 64);
        let mut encoder = Encoder::new().unwrap();
        let encoded = encoder.encode(&frame).unwrap().to_vec();
        assert!(!encoded.is_empty());

        let (server, probe, mut events) = gfx_server().await;
        let sink = Arc::new(Sink::default());
        let connection = open(&server, true, sink.clone()).await;
        session(connection, async {
            negotiate(&mut events).await;
            let region = Rect {
                x: 17,
                y: 29,
                w: 64,
                h: 64,
            };
            let surface_id = probe.surface(region);
            let frame_id = probe
                .server
                .lock()
                .unwrap()
                .send_avc420_frame(
                    surface_id,
                    &encoded,
                    &[Avc420Region::full_frame(64, 64, 0)],
                    0,
                )
                .unwrap();
            probe.flush();
            sink.assert_region(region, COLOR, 8).await;
            assert!(matches!(events.recv().await, Some(GfxEvent::FrameAck(id)) if id == frame_id));
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn server_without_gfx_falls_back() {
    wall_clock(async {
        let server = FakeRdpServer::start(FakeSecurity::Tls, WIDTH, HEIGHT)
            .await
            .unwrap();
        let sink = Arc::new(Sink::default());
        let connection = open(&server, true, sink.clone()).await;
        session(connection, bitmap(&server, &sink)).await;
    })
    .await;
}
