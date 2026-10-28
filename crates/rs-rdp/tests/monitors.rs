#![cfg(feature = "testkit")]

use std::any::TypeId;
use std::borrow::Cow;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ironrdp_core::encode_vec;
use ironrdp_displaycontrol::pdu::{
    DisplayControlCapabilities, DisplayControlMonitorLayout, DisplayControlPdu,
};
use ironrdp_dvc::pdu::{CreateRequestPdu, DataPdu, DrdynvcDataPdu, DrdynvcServerPdu};
use ironrdp_dvc::DrdynvcClient;
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_pdu::gcc::{Monitor, MonitorFlags};
use ironrdp_pdu::mcs::{McsMessage, SendDataIndication};
use ironrdp_pdu::rdp::client_info::CompressionType;
use ironrdp_pdu::rdp::finalization_messages::MonitorLayoutPdu;
use ironrdp_pdu::rdp::headers::{
    CompressionFlags, ShareControlHeader, ShareControlPdu, ShareDataHeader, ShareDataPdu,
    StreamPriority,
};
use ironrdp_pdu::x224::X224;
use ironrdp_pdu::Action;
use ironrdp_server::{
    ConnectionHandler, ConnectionInfo, ConnectionSource, DesktopSize, DisplayContext,
    DisplayUpdate, IncomingConnection, RdpServer, RdpServerDisplay, RdpServerDisplayUpdates,
    ServerResult, TransportTls,
};
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput};
use ironrdp_svc::{StaticChannelSet, SvcProcessor as _};
use rcgen::{CertificateParams, KeyPair};
use rs_rdp::client::{connect, RdpConnection};
use rs_rdp::display;
use rs_rdp::ext::ExtCtx;
use rs_rdp::{CertCheck, RdpCommand, RdpConfig, RdpError, RdpSink, Rect};
use rustls::pki_types::PrivatePkcs8KeyDer;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex, Notify};

const STEP: Duration = Duration::from_secs(5);

fn stalled<T>(what: &'static str) -> impl FnOnce(tokio::time::error::Elapsed) -> T {
    move |_| panic!("monitor count change stalled: {what}")
}

async fn wall_clock(test: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(20), test)
        .await
        .expect("monitor test exceeded its 20-second wall-clock budget");
}

#[derive(Default)]
struct RecSink {
    sizes: Mutex<Vec<(u16, u16)>>,
    layouts: Mutex<Vec<Vec<Rect>>>,
    changed: Notify,
}

impl RecSink {
    async fn layouts(&self, count: usize) -> Vec<Vec<Rect>> {
        loop {
            let notified = self.changed.notified();
            let layouts = self.layouts.lock().unwrap().clone();
            if layouts.len() >= count {
                return layouts;
            }
            notified.await;
        }
    }

    async fn size(&self, width: u16, height: u16) {
        tokio::time::timeout(STEP, async {
            loop {
                let changed = self.changed.notified();
                if self.sizes.lock().unwrap().last() == Some(&(width, height)) {
                    return;
                }
                changed.await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "desktop did not become {width}x{height}: {:?}",
                self.sizes.lock().unwrap()
            )
        });
    }
}

impl RdpSink for RecSink {
    fn resize(&self, w: u16, h: u16) {
        self.sizes.lock().unwrap().push((w, h));
        self.changed.notify_one();
    }
    fn update(&self, _: &[u8], _: usize, _: Rect) {}
    fn commit(&self) {}
    fn pointer_bitmap(&self, _: u16, _: u16, _: u16, _: u16, _: &[u8]) {}
    fn pointer_hidden(&self) {}
    fn pointer_default(&self) {}
    fn pointer_pos(&self, _: u16, _: u16) {}
    fn monitors(&self, layout: &[Rect]) {
        self.layouts.lock().unwrap().push(layout.to_vec());
        self.changed.notify_one();
    }
}

struct AcceptAll;

#[async_trait]
impl CertCheck for AcceptAll {
    async fn check(&self, _: [u8; 32]) -> bool {
        true
    }
}

fn config(monitors: u8) -> RdpConfig {
    RdpConfig {
        server_name: "127.0.0.1".into(),
        credentials: None,
        width: 1024,
        height: 768,
        scale_percent: 100,
        client_name: "monitor-test".into(),
        cert: Arc::new(AcceptAll),
        gfx: false,
        monitors,
        audio: None,
        clipboard: false,
    }
}

// The shared testkit facade cannot observe GCC data or replace its display
// handler. Keep this display-specific server and its lifecycle in this test.
struct MonitorServer {
    addr: SocketAddr,
    declarations: mpsc::UnboundedReceiver<Vec<Monitor>>,
    layouts: mpsc::UnboundedReceiver<DisplayControlMonitorLayout>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct ConnectionProbe(mpsc::UnboundedSender<Vec<Monitor>>);

impl ConnectionHandler for ConnectionProbe {
    fn on_connection_info(&mut self, info: &ConnectionInfo) {
        self.0.send(info.client_monitors.clone()).unwrap();
    }
}

struct Display {
    size: DesktopSize,
    layouts: mpsc::UnboundedSender<DisplayControlMonitorLayout>,
    updates_tx: mpsc::UnboundedSender<DisplayUpdate>,
    updates: Arc<AsyncMutex<mpsc::UnboundedReceiver<DisplayUpdate>>>,
    display_control: bool,
    monitor_count: u32,
}

struct Updates(Arc<AsyncMutex<mpsc::UnboundedReceiver<DisplayUpdate>>>);

#[async_trait]
impl RdpServerDisplayUpdates for Updates {
    async fn next_update(&mut self) -> ServerResult<Option<DisplayUpdate>> {
        Ok(self.0.lock().await.recv().await)
    }
}

#[async_trait]
impl RdpServerDisplay for Display {
    async fn size(&mut self) -> DesktopSize {
        self.size
    }

    async fn request_initial_size(&mut self, size: DesktopSize) -> DesktopSize {
        self.size = size;
        size
    }

    async fn updates(
        &mut self,
        _: DisplayContext,
    ) -> ServerResult<Box<dyn RdpServerDisplayUpdates>> {
        Ok(Box::new(Updates(self.updates.clone())))
    }

    fn request_layout(&mut self, layout: DisplayControlMonitorLayout) {
        let (width, height) = layout.desktop_size();
        self.size = DesktopSize {
            width: u16::try_from(width).unwrap(),
            height: u16::try_from(height).unwrap(),
        };
        self.updates_tx
            .send(DisplayUpdate::Resize(self.size))
            .unwrap();
        self.layouts.send(layout).unwrap();
    }

    async fn monitor_count(&mut self) -> u32 {
        self.monitor_count
    }

    async fn offers_display_control(&mut self) -> bool {
        self.display_control
    }
}

struct Source(TcpListener);

#[async_trait]
impl ConnectionSource for Source {
    type Stream = TcpStream;

    async fn accept(&mut self) -> io::Result<Option<IncomingConnection<TcpStream>>> {
        let (stream, peer) = self.0.accept().await?;
        stream.set_nodelay(true)?;
        Ok(Some(IncomingConnection::new(
            stream,
            TransportTls::Managed,
            Some(peer),
        )))
    }
}

impl MonitorServer {
    fn start(display_control: bool, monitor_count: u32) -> Self {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let key = KeyPair::generate().unwrap();
        let certificate = CertificateParams::new(vec!["hs.example.com".into()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.der().clone()],
            PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .unwrap();
        let (declarations_tx, declarations) = mpsc::unbounded_channel();
        let (layouts_tx, layouts) = mpsc::unbounded_channel();
        let (updates_tx, updates) = mpsc::unbounded_channel();
        let (stop, stopped) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("rdp-monitors".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(tokio::task::LocalSet::new().run_until(async move {
                    let mut source = Source(TcpListener::from_std(listener).unwrap());
                    let mut server = RdpServer::builder()
                        .with_addr(addr)
                        .with_tls(tokio_rustls::TlsAcceptor::from(Arc::new(tls)))
                        .with_no_input()
                        .with_display_handler(Display {
                            size: DesktopSize {
                                width: 1024,
                                height: 768,
                            },
                            layouts: layouts_tx,
                            updates_tx,
                            updates: Arc::new(AsyncMutex::new(updates)),
                            display_control,
                            monitor_count,
                        })
                        .with_connection_handler(Some(Box::new(ConnectionProbe(declarations_tx))))
                        .with_honor_client_desktop_size(Some(DesktopSize {
                            width: 8192,
                            height: 8192,
                        }))
                        .build();
                    tokio::select! {
                        result = server.serve(&mut source) => result.unwrap(),
                        _ = stopped => {}
                    }
                }));
            })
            .unwrap();
        Self {
            addr,
            declarations,
            layouts,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    async fn connect(&self, cfg: RdpConfig, sink: Arc<RecSink>) -> RdpConnection {
        let stream = TcpStream::connect(self.addr).await.unwrap();
        stream.set_nodelay(true).unwrap();
        connect(stream, cfg, sink).await.unwrap()
    }

    async fn resized(
        &mut self,
        commands: &mpsc::Sender<RdpCommand>,
        width: u16,
        height: u16,
        scale_percent: u32,
    ) -> DisplayControlMonitorLayout {
        let mut retry = tokio::time::interval(Duration::from_millis(10));
        loop {
            tokio::select! {
                layout = self.layouts.recv() => return layout.unwrap(),
                _ = retry.tick() => commands.send(RdpCommand::Resize {
                    width, height, scale_percent,
                }).await.unwrap(),
            }
        }
    }
}

impl Drop for MonitorServer {
    fn drop(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.thread.take().unwrap().join().unwrap();
    }
}

async fn session<F, Fut>(connection: RdpConnection, action: F)
where
    F: FnOnce(mpsc::Sender<RdpCommand>) -> Fut,
    Fut: Future<Output = ()>,
{
    let (tx, rx) = mpsc::channel(16);
    let (result, ()) = tokio::join!(connection.run(rx), action(tx));
    assert_eq!(result, Ok(()));
}

fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
    Rect { x, y, w, h }
}

fn monitor(left: i32, top: i32, right: i32, bottom: i32, primary: bool) -> Monitor {
    Monitor {
        left,
        top,
        right,
        bottom,
        flags: if primary {
            MonitorFlags::PRIMARY
        } else {
            MonitorFlags::empty()
        },
    }
}

#[tokio::test]
async fn two_monitors_declared() {
    wall_clock(async {
        let mut server = MonitorServer::start(true, 4);
        let sink = Arc::new(RecSink::default());
        let connection = server.connect(config(2), sink.clone()).await;
        assert_eq!(connection.desktop_size(), (2048, 768));
        session(connection, |commands| async {
            assert_eq!(
                server.declarations.recv().await.unwrap(),
                vec![
                    monitor(0, 0, 1023, 767, true),
                    monitor(1024, 0, 2047, 767, false),
                ]
            );
            assert_eq!(
                sink.layouts(1).await[0],
                vec![rect(0, 0, 1024, 768), rect(1024, 0, 1024, 768)]
            );
            assert_eq!(sink.sizes.lock().unwrap()[0], (2048, 768));
            drop(commands);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn single_monitor_reports_one() {
    wall_clock(async {
        let mut server = MonitorServer::start(true, 4);
        let sink = Arc::new(RecSink::default());
        let connection = server.connect(config(1), sink.clone()).await;
        session(connection, |commands| async {
            assert!(server.declarations.recv().await.unwrap().is_empty());
            assert_eq!(sink.layouts(1).await[0], vec![rect(0, 0, 1024, 768)]);
            drop(commands);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn clamp_count() {
    wall_clock(async {
        for (requested, expected) in [(9, 4), (0, 1)] {
            let mut server = MonitorServer::start(true, 4);
            let sink = Arc::new(RecSink::default());
            let connection = server.connect(config(requested), sink.clone()).await;
            session(connection, |commands| async {
                let declared = server.declarations.recv().await.unwrap();
                if expected == 1 {
                    assert!(declared.is_empty());
                } else {
                    assert_eq!(declared.len(), expected);
                    assert_eq!(declared[3], monitor(3072, 0, 4095, 767, false));
                }
                assert_eq!(sink.layouts(1).await[0].len(), expected);
                drop(commands);
            })
            .await;
        }
    })
    .await;
}

#[tokio::test]
async fn resize_all_monitors() {
    wall_clock(async {
        let mut server = MonitorServer::start(true, 4);
        let sink = Arc::new(RecSink::default());
        let connection = server.connect(config(2), sink.clone()).await;
        session(connection, |commands| async {
            sink.layouts(1).await;
            let layout = server.resized(&commands, 800, 600, 100).await;
            let monitors = layout.monitors();
            assert_eq!(monitors.len(), 2);
            assert!(monitors[0].is_primary());
            assert!(!monitors[1].is_primary());
            assert_eq!(monitors[0].position(), Some((0, 0)));
            assert_eq!(monitors[1].position(), Some((800, 0)));
            for monitor in monitors {
                assert_eq!(monitor.dimensions(), (800, 600));
                assert_eq!(monitor.desktop_scale_factor(), Some(100));
            }
            assert_eq!(layout.desktop_size(), (1600, 600));
            drop(commands);
        })
        .await;
    })
    .await;
}

fn empty_stage() -> ActiveStage {
    stage(StaticChannelSet::new())
}

fn stage(static_channels: StaticChannelSet) -> ActiveStage {
    ActiveStageBuilder {
        static_channels,
        user_channel_id: 1001,
        io_channel_id: 1003,
        message_channel_id: None,
        share_id: 1,
        compression_type: None,
        enable_server_pointer: false,
        pointer_software_rendering: false,
    }
    .build()
}

fn display_stage(ctx: &ExtCtx, capabilities: bool) -> ActiveStage {
    let mut channel = display::dynamic_channels(&config(2), ctx, DrdynvcClient::new());
    channel
        .process(
            &encode_vec(&DrdynvcServerPdu::Create(CreateRequestPdu::new(
                1,
                ironrdp_displaycontrol::CHANNEL_NAME.to_owned(),
            )))
            .unwrap(),
        )
        .unwrap();
    if capabilities {
        let caps = encode_vec(&DisplayControlPdu::Caps(
            DisplayControlCapabilities::new(4, 8192, 8192).unwrap(),
        ))
        .unwrap();
        channel
            .process(
                &encode_vec(&DrdynvcServerPdu::Data(DrdynvcDataPdu::Data(DataPdu::new(
                    1, caps,
                ))))
                .unwrap(),
            )
            .unwrap();
    }
    let mut channels = StaticChannelSet::new();
    channels.insert(channel);
    channels.attach_channel_id(TypeId::of::<DrdynvcClient>(), 1004);
    stage(channels)
}

fn context(sink: Arc<RecSink>) -> ExtCtx {
    ExtCtx {
        sink,
        audio: None,
        wake: Arc::new(Notify::new()),
        background: Default::default(),
    }
}

fn receive_layout(stage: &mut ActiveStage, ctx: &ExtCtx, monitors: Vec<Monitor>) {
    let data = encode_vec(&ShareControlHeader {
        pdu_source: 1001,
        share_id: 1,
        share_control_pdu: ShareControlPdu::Data(ShareDataHeader {
            share_data_pdu: ShareDataPdu::MonitorLayout(MonitorLayoutPdu { monitors }),
            stream_priority: StreamPriority::Medium,
            compression_flags: CompressionFlags::empty(),
            compression_type: CompressionType::K8,
        }),
    })
    .unwrap();
    let frame = encode_vec(&X224(McsMessage::SendDataIndication(SendDataIndication {
        initiator_id: 1001,
        channel_id: 1003,
        user_data: Cow::Owned(data),
    })))
    .unwrap();
    let mut image = DecodedImage::new(PixelFormat::RgbA32, 2048, 1536);
    let outputs = stage
        .process(&mut image, Action::X224, &frame, None)
        .unwrap();
    let mut received = 0;
    for output in outputs {
        if let ActiveStageOutput::MonitorLayout(monitors) = output {
            display::server_layout(ctx, &monitors);
            received += 1;
        }
    }
    assert_eq!(received, 1, "server Monitor Layout PDU was not dispatched");
}

#[tokio::test]
async fn server_layout_wins() {
    wall_clock(async {
        let server = MonitorServer::start(true, 4);
        let sink = Arc::new(RecSink::default());
        let _connection = server.connect(config(2), sink.clone()).await;
        let ctx = context(sink.clone());
        let mut stage = empty_stage();
        assert!(display::pump(&mut stage, &ctx).unwrap().is_empty());
        receive_layout(
            &mut stage,
            &ctx,
            vec![
                monitor(0, 0, 1023, 767, true),
                monitor(0, 768, 1023, 1535, false),
            ],
        );
        assert_eq!(
            sink.layouts(2).await,
            vec![
                vec![rect(0, 0, 1024, 768), rect(1024, 0, 1024, 768)],
                vec![rect(0, 0, 1024, 768), rect(0, 768, 1024, 768)],
            ]
        );
        display::pump(&mut stage, &ctx).unwrap();
        assert_eq!(sink.layouts.lock().unwrap().len(), 2);
    })
    .await;
}

#[tokio::test]
async fn activation_layout_is_normalized_and_primary_first() {
    wall_clock(async {
        let server = MonitorServer::start(true, 4);
        let sink = Arc::new(RecSink::default());
        let _connection = server.connect(config(2), sink.clone()).await;
        let ctx = context(sink.clone());
        let mut stage = empty_stage();
        receive_layout(
            &mut stage,
            &ctx,
            vec![
                monitor(-1024, -768, -1, -1, false),
                monitor(0, 0, 1023, 767, true),
            ],
        );
        assert!(sink.layouts.lock().unwrap().is_empty());
        display::pump(&mut stage, &ctx).unwrap();
        assert_eq!(
            sink.layouts(2).await[1],
            vec![rect(1024, 768, 1024, 768), rect(0, 0, 1024, 768)]
        );
        display::pump(&mut stage, &ctx).unwrap();
        assert_eq!(sink.layouts.lock().unwrap().len(), 2);
    })
    .await;
}

#[tokio::test]
async fn single_monitor_resize_still_reactivates() {
    wall_clock(async {
        let mut server = MonitorServer::start(true, 4);
        let sink = Arc::new(RecSink::default());
        let connection = server.connect(config(1), sink.clone()).await;
        session(connection, |commands| async {
            sink.layouts(2).await;
            let layout = server.resized(&commands, 801, 600, 600).await;
            assert_eq!(layout.desktop_size(), (800, 600));
            assert_eq!(layout.monitors().len(), 1);
            assert_eq!(layout.monitors()[0].desktop_scale_factor(), Some(500));
            sink.layouts(3).await;
            assert_eq!(sink.sizes.lock().unwrap().last(), Some(&(800, 600)));
            drop(commands);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn resize_without_display_control_is_ignored() {
    wall_clock(async {
        let server = MonitorServer::start(false, 4);
        let sink = Arc::new(RecSink::default());
        let _connection = server.connect(config(2), sink.clone()).await;
        let mut stage = empty_stage();
        let ctx = context(sink.clone());
        assert!(display::resize(&mut stage, &ctx, 800, 600, 100)
            .unwrap()
            .is_empty());
        let mut stage = display_stage(&ctx, false);
        assert!(display::resize(&mut stage, &ctx, 800, 600, 100)
            .unwrap()
            .is_empty());
        display::pump(&mut stage, &ctx).unwrap();
        assert_eq!(
            sink.layouts(1).await[0],
            vec![rect(0, 0, 1024, 768), rect(1024, 0, 1024, 768)]
        );
    })
    .await;
}

#[tokio::test]
async fn repeated_resize_is_ignored_but_topology_and_scale_changes_are_sent() {
    wall_clock(async {
        let server = MonitorServer::start(true, 4);
        let sink = Arc::new(RecSink::default());
        let _connection = server.connect(config(2), sink.clone()).await;
        let ctx = context(sink);
        let mut stage = display_stage(&ctx, true);
        display::pump(&mut stage, &ctx).unwrap();
        assert!(display::resize(&mut stage, &ctx, 1024, 768, 100)
            .unwrap()
            .is_empty());
        display::server_layout(&ctx, &[monitor(0, 0, 2047, 767, true)]);
        assert!(!display::resize(&mut stage, &ctx, 1024, 768, 100)
            .unwrap()
            .is_empty());
        assert!(display::resize(&mut stage, &ctx, 1024, 768, 100)
            .unwrap()
            .is_empty());
        assert!(!display::resize(&mut stage, &ctx, 1024, 768, 150)
            .unwrap()
            .is_empty());
        assert!(display::resize(&mut stage, &ctx, 1024, 768, 150)
            .unwrap()
            .is_empty());
    })
    .await;
}

#[tokio::test]
async fn resize_respects_server_monitor_limit() {
    wall_clock(async {
        let mut server = MonitorServer::start(true, 1);
        let sink = Arc::new(RecSink::default());
        let connection = server.connect(config(2), sink.clone()).await;
        session(connection, |commands| async {
            sink.layouts(1).await;
            let layout = server.resized(&commands, 800, 600, 100).await;
            assert_eq!(layout.monitors().len(), 1);
            assert!(layout.monitors()[0].is_primary());
            assert_eq!(layout.desktop_size(), (800, 600));
            drop(commands);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn invalid_server_layout_does_not_replace_current_layout() {
    wall_clock(async {
        let server = MonitorServer::start(true, 4);
        let sink = Arc::new(RecSink::default());
        let _connection = server.connect(config(2), sink.clone()).await;
        let ctx = context(sink.clone());
        let mut stage = empty_stage();
        display::pump(&mut stage, &ctx).unwrap();
        for monitors in [
            vec![],
            vec![monitor(10, 0, 0, 767, true)],
            vec![monitor(i32::MIN, 0, i32::MAX, 767, true)],
        ] {
            display::server_layout(&ctx, &monitors);
        }
        assert_eq!(sink.layouts.lock().unwrap().len(), 1);
    })
    .await;
}

#[tokio::test]
async fn monitor_count_changes_at_runtime() {
    wall_clock(async {
        let mut server = MonitorServer::start(true, 4);
        let sink = Arc::new(RecSink::default());
        let connection = server.connect(config(1), sink.clone()).await;
        let (commands, receiver) = mpsc::channel(16);
        let run = connection.run(receiver);
        tokio::pin!(run);
        let change = async {
            sink.layouts(1).await;
            // Display control is up once a resize goes through.
            tokio::time::timeout(STEP, server.resized(&commands, 800, 600, 100))
                .await
                .unwrap_or_else(stalled("resize"));
            // Ask once the server has finished the reactivation for it.
            sink.size(800, 600).await;
            commands.send(RdpCommand::Monitors(2)).await.unwrap();
            let layout = tokio::time::timeout(STEP, server.layouts.recv())
                .await
                .unwrap_or_else(stalled("two-monitor layout"))
                .unwrap();
            let monitors = layout.monitors();
            assert_eq!(monitors.len(), 2);
            assert!(monitors[0].is_primary());
            assert!(!monitors[1].is_primary());
            assert_eq!(monitors[0].position(), Some((0, 0)));
            assert_eq!(monitors[1].position(), Some((800, 0)));
            for monitor in monitors {
                assert_eq!(monitor.dimensions(), (800, 600));
            }
            assert_eq!(layout.desktop_size(), (1600, 600));
            sink.size(1600, 600).await;
        };
        tokio::select! {
            result = &mut run => panic!("connection ended early: {result:?}"),
            () = change => {}
        }
        drop(commands);
        assert_eq!(run.await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn monitor_count_without_display_control_asks_to_reconnect() {
    wall_clock(async {
        let server = MonitorServer::start(false, 4);
        let sink = Arc::new(RecSink::default());
        let connection = server.connect(config(1), sink.clone()).await;
        let (commands, receiver) = mpsc::channel(16);
        // Unchanged: nothing to do. More than four: clamped.
        commands.send(RdpCommand::Monitors(1)).await.unwrap();
        commands.send(RdpCommand::Monitors(9)).await.unwrap();
        assert_eq!(connection.run(receiver).await, Err(RdpError::Reconnect(4)));
    })
    .await;
}

#[tokio::test]
async fn set_monitors_falls_back_without_display_control() {
    wall_clock(async {
        let server = MonitorServer::start(true, 4);
        let sink = Arc::new(RecSink::default());
        let _connection = server.connect(config(2), sink.clone()).await;
        let ctx = context(sink);
        let mut stage = empty_stage();
        assert_eq!(
            display::set_monitors(&mut stage, &ctx, 2).unwrap(),
            Some(Vec::new())
        );
        assert_eq!(display::set_monitors(&mut stage, &ctx, 3).unwrap(), None);
        let mut stage = display_stage(&ctx, false);
        assert_eq!(display::set_monitors(&mut stage, &ctx, 1).unwrap(), None);
        let mut stage = display_stage(&ctx, true);
        assert!(!display::set_monitors(&mut stage, &ctx, 1)
            .unwrap()
            .unwrap()
            .is_empty());
        assert_eq!(
            display::set_monitors(&mut stage, &ctx, 1).unwrap(),
            Some(Vec::new())
        );
        assert!(!display::set_monitors(&mut stage, &ctx, 3)
            .unwrap()
            .unwrap()
            .is_empty());
        // Once the server has opened display control, a request made while a
        // reactivation closed it waits for the channel instead of reconnecting.
        display::pump(&mut stage, &ctx).unwrap();
        let mut closed = empty_stage();
        assert_eq!(
            display::set_monitors(&mut closed, &ctx, 2).unwrap(),
            Some(Vec::new())
        );
        assert!(display::pump(&mut closed, &ctx).unwrap().is_empty());
        assert!(!display::pump(&mut stage, &ctx).unwrap().is_empty());
        assert!(display::pump(&mut stage, &ctx).unwrap().is_empty());
    })
    .await;
}
