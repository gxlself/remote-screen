#[allow(dead_code)]
mod support;

use std::time::Duration;

use rsp_client::session::{Session, SessionHandle, SessionParams, SessionStateKind};
use rsp_client::viewport::ViewportInput;
use rsp_client::{EventStream, SessionEvent};
use rsp_core::{features, pb};
use rsp_transport::identity::{CertKind, LocalIdentity};
use rsp_transport::{Conn, ControlStream};
use tokio::time::{sleep, timeout, timeout_at, Instant};

struct TestSession {
    conn: Conn,
    control: ControlStream,
    handle: SessionHandle,
    events: EventStream,
    hello: pb::ClientHello,
}

impl TestSession {
    async fn new(
        conn: Conn,
        control: ControlStream,
        handle: SessionHandle,
        events: EventStream,
    ) -> Self {
        let pb::envelope::Msg::ClientHello(hello) = support::recv(&control).await else {
            panic!("expected ClientHello");
        };
        Self {
            conn,
            control,
            handle,
            events,
            hello,
        }
    }

    async fn start(&mut self, features: u64) {
        support::send(
            &self.control,
            pb::ServerHello {
                protocol: features::PROTOCOL_VERSION,
                features,
                ..Default::default()
            },
        )
        .await;
        self.authenticate().await;
    }

    async fn authenticate(&mut self) {
        support::auth(&self.control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv_command(&self.control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        loop {
            if matches!(
                self.next_event().await,
                SessionEvent::StateChanged(SessionStateKind::Streaming)
            ) {
                break;
            }
        }
    }

    async fn next_event(&mut self) -> SessionEvent {
        let event = timeout(support::TEST_TIMEOUT, self.events.recv())
            .await
            .expect("session event deadline")
            .expect("session event");
        assert!(
            !matches!(event, SessionEvent::Ended { .. }),
            "session ended unexpectedly: {event:?}"
        );
        event
    }

    async fn expect_no_viewport(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while let Ok(message) = timeout_at(deadline, support::recv_command(&self.control)).await {
            assert!(
                !matches!(message, pb::envelope::Msg::Viewport(_)),
                "unexpected viewport: {message:?}"
            );
        }
    }

    async fn next_viewport(&self) -> pb::Viewport {
        timeout(Duration::from_secs(5), async {
            loop {
                if let pb::envelope::Msg::Viewport(viewport) =
                    support::recv_command(&self.control).await
                {
                    return viewport;
                }
            }
        })
        .await
        .expect("viewport deadline")
    }

    async fn settled_viewport(&self, mut viewport: pb::Viewport) -> pb::Viewport {
        timeout(Duration::from_secs(1), async {
            while !viewport.settled {
                viewport = self.next_viewport().await;
            }
            viewport
        })
        .await
        .expect("settled viewport deadline")
    }

    async fn initial_viewport(&self) -> pb::Viewport {
        self.handle.set_viewport(q(0.25, 0.25, 0.5, 0.5, false));
        self.expect_no_viewport(Duration::from_millis(300)).await;
        support::send(&self.control, cfg(1, 1920, 1080)).await;
        let viewport = self.next_viewport().await;
        assert_eq!(viewport.seq, 1);
        assert_eq!(viewport.display_id, 1);
        assert_eq!(
            viewport.rect,
            Some(pb::PixelRect {
                x: 480,
                y: 256,
                w: 960,
                h: 560,
            })
        );
        assert_eq!(viewport.view_width_px, 2528);
        assert_eq!(viewport.view_height_px, 1168);
        assert!(viewport.crop_ok);
        self.settled_viewport(viewport).await
    }

    async fn finish(&mut self) {
        support::finish(&self.conn, &self.control).await;
        timeout(support::TEST_TIMEOUT, async {
            loop {
                if let SessionEvent::Ended { code, .. } =
                    self.events.recv().await.expect("session end")
                {
                    assert_eq!(code, None);
                    break;
                }
            }
        })
        .await
        .expect("session end deadline");
    }
}

async fn open_quic(host: &support::Host, advertise_viewport: bool) -> TestSession {
    let mut params = SessionParams::single(
        host.addr,
        host.fingerprint,
        LocalIdentity::generate_software(CertKind::App).expect("App identity"),
    );
    if !advertise_viewport {
        params.features &= !features::VIEWPORT;
    }
    let (client, (conn, control)) = tokio::join!(Session::open(params), host.accept());
    let (handle, events) = client.expect("open session");
    TestSession::new(conn, control, handle, events).await
}

fn q(x: f32, y: f32, w: f32, h: f32, active: bool) -> ViewportInput {
    ViewportInput {
        x,
        y,
        w,
        h,
        view_w_px: 2532,
        view_h_px: 1170,
        active,
        crop_ok: true,
    }
}

fn cfg(display: u32, w: u32, h: u32) -> pb::StreamConfig {
    pb::StreamConfig {
        epoch: 1,
        display_id: display,
        display_width_px: w,
        display_height_px: h,
        codec: pb::Codec::Hevc as i32,
        width: 1280,
        height: 720,
        fps: 30,
        ..Default::default()
    }
}

#[tokio::test]
async fn hello_advertises_viewport() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, true).await;
    assert_ne!(session.hello.features & features::VIEWPORT, 0);
    assert_eq!(session.handle.viewport_mode(), pb::viewport_ack::Mode::Off);
    session.start(0).await;
    session.finish().await;
}

#[tokio::test]
async fn silent_without_host_feature() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, true).await;
    session.start(0).await;
    support::send(&session.control, cfg(1, 1920, 1080)).await;
    session.handle.set_viewport(q(0.25, 0.25, 0.5, 0.5, false));
    session.expect_no_viewport(Duration::from_secs(1)).await;
    session.finish().await;
}

#[tokio::test]
async fn silent_without_client_feature() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, false).await;
    assert_eq!(session.hello.features & features::VIEWPORT, 0);
    session.start(features::VIEWPORT).await;
    support::send(&session.control, cfg(1, 1920, 1080)).await;
    session.handle.set_viewport(q(0.25, 0.25, 0.5, 0.5, false));
    session.expect_no_viewport(Duration::from_secs(1)).await;
    session.finish().await;
}

#[tokio::test]
async fn waits_for_stream_config() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, true).await;
    session.start(features::VIEWPORT).await;
    session.initial_viewport().await;
    session.expect_no_viewport(Duration::from_millis(500)).await;
    session.finish().await;
}

#[tokio::test]
async fn input_before_streaming_is_retained() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, true).await;
    let handle = session.handle.clone();
    std::thread::spawn(move || handle.set_viewport(q(0.25, 0.25, 0.5, 0.5, false)))
        .join()
        .expect("viewport caller thread");
    session.expect_no_viewport(Duration::from_millis(300)).await;
    session.start(features::VIEWPORT).await;
    support::send(&session.control, cfg(1, 1920, 1080)).await;
    let viewport = session.next_viewport().await;
    assert_eq!(viewport.seq, 1);
    assert_eq!(viewport.display_id, 1);
    assert_eq!(
        viewport.rect,
        Some(pb::PixelRect {
            x: 480,
            y: 256,
            w: 960,
            h: 560,
        })
    );
    session.finish().await;
}

#[tokio::test]
async fn missing_display_size_silent() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, true).await;
    session.start(features::VIEWPORT).await;
    support::send(&session.control, cfg(1, 0, 0)).await;
    session.handle.set_viewport(q(0.25, 0.25, 0.5, 0.5, false));
    session.expect_no_viewport(Duration::from_secs(1)).await;
    session.finish().await;
}

#[tokio::test]
async fn ack_sets_mode() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, true).await;
    session.start(features::VIEWPORT).await;
    let initial = session.initial_viewport().await;
    session.handle.set_viewport(q(0.5, 0.25, 0.5, 0.5, false));
    let viewport = session.next_viewport().await;
    let viewport = session.settled_viewport(viewport).await;
    assert!(viewport.seq > initial.seq);
    support::send(
        &session.control,
        pb::ViewportAck {
            seq: viewport.seq,
            mode: pb::viewport_ack::Mode::Crop as i32,
        },
    )
    .await;
    timeout(Duration::from_secs(2), async {
        while session.handle.viewport_mode() != pb::viewport_ack::Mode::Crop {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("viewport mode deadline");
    support::send(
        &session.control,
        pb::ViewportAck {
            seq: 1,
            mode: pb::viewport_ack::Mode::Off as i32,
        },
    )
    .await;
    session.expect_no_viewport(Duration::from_millis(300)).await;
    assert_eq!(session.handle.viewport_mode(), pb::viewport_ack::Mode::Crop);
    session.finish().await;
}

#[tokio::test]
async fn display_change_resends() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, true).await;
    session.start(features::VIEWPORT).await;
    let previous = session.initial_viewport().await;
    let mut config = cfg(2, 3840, 2160);
    config.epoch = 2;
    support::send(&session.control, config).await;
    let viewport = session.next_viewport().await;
    assert_eq!(viewport.display_id, 2);
    assert_eq!(
        viewport.rect,
        Some(pb::PixelRect {
            x: 960,
            y: 528,
            w: 1920,
            h: 1104,
        })
    );
    assert_eq!(viewport.seq, previous.seq + 1);
    session.finish().await;
}

#[tokio::test]
async fn invalid_input_ignored() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, true).await;
    session.start(features::VIEWPORT).await;
    support::send(&session.control, cfg(1, 1920, 1080)).await;
    session
        .handle
        .set_viewport(q(f32::NAN, 0.0, 1.0, 1.0, false));
    session.handle.set_viewport(q(0.0, 0.0, 0.0, 1.0, false));
    session.expect_no_viewport(Duration::from_secs(1)).await;
    session.finish().await;
}

#[tokio::test]
async fn full_screen_has_no_rect() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, true).await;
    session.start(features::VIEWPORT).await;
    session.handle.set_viewport(q(0.0, 0.0, 1.0, 1.0, false));
    support::send(&session.control, cfg(1, 1920, 1080)).await;
    assert_eq!(session.next_viewport().await.rect, None);
    session.finish().await;
}

#[tokio::test]
async fn ack_before_streaming_ignored() {
    let host = support::Host::bind().await;
    let mut session = open_quic(&host, true).await;
    support::send(
        &session.control,
        pb::ServerHello {
            protocol: features::PROTOCOL_VERSION,
            features: features::VIEWPORT,
            ..Default::default()
        },
    )
    .await;
    support::send(
        &session.control,
        pb::ViewportAck {
            seq: 1,
            mode: pb::viewport_ack::Mode::Crop as i32,
        },
    )
    .await;
    session.authenticate().await;
    assert_eq!(session.handle.state(), SessionStateKind::Streaming);
    assert_eq!(session.handle.viewport_mode(), pb::viewport_ack::Mode::Off);
    session.finish().await;
}
