#![cfg(feature = "testkit")]

// Uses connector + tokio + rustls directly, with no dependency on the app client.
#[allow(dead_code)]
#[path = "../examples/size_probe.rs"]
mod client;

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use client::{config, ProbeClient, ProbeError};
use ironrdp_connector::ConnectorErrorKind;
use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
use ironrdp_pdu::input::mouse::PointerFlags;
use ironrdp_pdu::input::MousePdu;
use ironrdp_pdu::nego::FailureCode;
use ironrdp_session::ActiveStageOutput;
use rs_rdp::testkit::{FakeInput, FakeRdpServer, FakeSecurity};
use sha2::{Digest, Sha256};

async fn wall_clock(test: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(20), test)
        .await
        .expect("probe exceeded its 20-second wall-clock budget");
}

fn hybrid() -> FakeSecurity {
    FakeSecurity::Hybrid {
        user: "alice".into(),
        password: "pw-1".into(),
        domain: None,
    }
}

#[tokio::test]
async fn tls_only_connects_and_receives_bitmap() {
    wall_clock(async {
        let server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        // Queued before activation: a producer does not need a connection-ready signal.
        server.push_bitmap(0, 0, 16, 16, [0, 0, 255, 255].repeat(16 * 16));
        let mut client = ProbeClient::connect(server.addr(), config("", "", None, false))
            .await
            .unwrap();
        assert_eq!((client.image.width(), client.image.height()), (1024, 768));
        loop {
            let outputs = client.next().await.unwrap();
            if outputs
                .iter()
                .any(|output| matches!(output, ActiveStageOutput::GraphicsUpdate(_)))
            {
                let offset = 8 * client.image.stride() + 8 * 4;
                assert_eq!(&client.image.data()[offset..offset + 4], &[255, 0, 0, 255]);
                break;
            }
        }
    })
    .await;
}

#[tokio::test]
async fn hybrid_good_credentials() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid(), 1024, 768).await.unwrap();
        let client = ProbeClient::connect(server.addr(), config("alice", "pw-1", None, true))
            .await
            .unwrap();
        assert_eq!(client.image.width(), 1024);
    })
    .await;
}

fn finalize_failure(result: Result<ProbeClient, ProbeError>) {
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("bad credentials reached ActiveStage"),
    };
    println!(
        "bad credentials: type={}, debug={error:?}, display={error}",
        std::any::type_name::<ProbeError>()
    );
    assert!(
        matches!(error, ProbeError::Finalize(_)),
        "failure must occur in connect_finalize"
    );
}

#[tokio::test]
async fn hybrid_bad_password_fails() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid(), 1024, 768).await.unwrap();
        finalize_failure(
            ProbeClient::connect(server.addr(), config("alice", "wrong", None, true)).await,
        );
    })
    .await;
}

#[tokio::test]
async fn hybrid_required_without_credssp() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid(), 1024, 768).await.unwrap();
        let error = match ProbeClient::connect(server.addr(), config("", "", None, false)).await {
            Err(error) => error,
            Ok(_) => panic!("TLS-only client connected to a HYBRID-only server"),
        };
        println!("NLA required: debug={error:?}, display={error}");
        let ProbeError::Begin(error) = error else {
            panic!("must fail at negotiation");
        };
        let ConnectorErrorKind::Negotiation(failure) = error.kind() else {
            panic!("expected RDP_NEG_FAILURE: {error:?}");
        };
        assert_eq!(failure.code(), FailureCode::HYBRID_REQUIRED_BY_SERVER);
    })
    .await;
}

#[tokio::test]
async fn input_reaches_server() {
    wall_clock(async {
        let mut server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let mut client = ProbeClient::connect(server.addr(), config("", "", None, false))
            .await
            .unwrap();
        client
            .send(&[FastPathInputEvent::KeyboardEvent(
                KeyboardFlags::empty(),
                0x1e,
            )])
            .await
            .unwrap();
        assert_eq!(
            server.next_input(Duration::from_secs(5)).await,
            Some(FakeInput::Key {
                scancode: 0x1e,
                extended: false,
                down: true
            })
        );
        client
            .send(&[FastPathInputEvent::MouseEvent(MousePdu {
                flags: PointerFlags::MOVE,
                number_of_wheel_rotation_units: 0,
                x_position: 10,
                y_position: 20,
            })])
            .await
            .unwrap();
        assert_eq!(
            server.next_input(Duration::from_secs(5)).await,
            Some(FakeInput::MouseMove { x: 10, y: 20 })
        );
        client
            .send(&[
                FastPathInputEvent::KeyboardEvent(
                    KeyboardFlags::EXTENDED | KeyboardFlags::RELEASE,
                    0x1d,
                ),
                FastPathInputEvent::UnicodeKeyboardEvent(KeyboardFlags::empty(), 0x4e2d),
                FastPathInputEvent::UnicodeKeyboardEvent(KeyboardFlags::RELEASE, 0x4e2d),
                FastPathInputEvent::MouseEvent(MousePdu {
                    flags: PointerFlags::RIGHT_BUTTON | PointerFlags::DOWN,
                    number_of_wheel_rotation_units: 0,
                    x_position: 10,
                    y_position: 20,
                }),
                FastPathInputEvent::MouseEvent(MousePdu {
                    flags: PointerFlags::HORIZONTAL_WHEEL,
                    number_of_wheel_rotation_units: -120,
                    x_position: 0,
                    y_position: 0,
                }),
            ])
            .await
            .unwrap();
        for expected in [
            FakeInput::Key {
                scancode: 0x1d,
                extended: true,
                down: false,
            },
            FakeInput::Unicode {
                code: 0x4e2d,
                down: true,
            },
            FakeInput::Unicode {
                code: 0x4e2d,
                down: false,
            },
            FakeInput::Button {
                button: 1,
                down: true,
            },
            FakeInput::Wheel {
                horizontal: true,
                units: -120,
            },
        ] {
            assert_eq!(
                server.next_input(Duration::from_secs(1)).await,
                Some(expected)
            );
        }
        assert_eq!(server.next_input(Duration::from_millis(10)).await, None);
    })
    .await;
}

async fn read_until_closed(client: &mut ProbeClient) -> String {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client.next().await {
                Ok(outputs) => {
                    for output in outputs {
                        if let ActiveStageOutput::Terminate(reason) = output {
                            return format!("ActiveStageOutput::Terminate({reason:?}): {reason}");
                        }
                    }
                }
                Err(error) => {
                    let io = error
                        .downcast_ref::<std::io::Error>()
                        .expect("disconnect must be termination or transport close");
                    assert!(matches!(
                        io.kind(),
                        std::io::ErrorKind::UnexpectedEof
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::BrokenPipe
                    ));
                    return format!("io::{:?}: {io}", io.kind());
                }
            }
        }
    })
    .await
    .expect("disconnect was not observed within 5 seconds")
}

#[tokio::test]
async fn server_disconnect_seen() {
    wall_clock(async {
        let server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let mut client = ProbeClient::connect(server.addr(), config("", "", None, false))
            .await
            .unwrap();
        server.disconnect();
        println!(
            "server disconnect: {}",
            read_until_closed(&mut client).await
        );
        // A retained pointer-injection sender must not keep the TLS socket open.
        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Err(error) = client.framed.read_pdu().await {
                    break error;
                }
            }
        })
        .await
        .expect("server left the disconnected socket open");
        assert!(matches!(
            closed.kind(),
            std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
        ));
        // Disconnect only ends the session: the same listener accepts the next client.
        drop(client);
        let _client = ProbeClient::connect(server.addr(), config("", "", None, false))
            .await
            .unwrap();
        assert_eq!(server.connections(), 2);
    })
    .await;
}

#[tokio::test]
async fn cert_fingerprint_matches() {
    wall_clock(async {
        let server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let client = ProbeClient::connect(server.addr(), config("", "", None, false))
            .await
            .unwrap();
        let actual: [u8; 32] = Sha256::digest(&client.leaf_der).into();
        assert_eq!(actual, server.cert_sha256());
        let (_, cert) = x509_parser::parse_x509_certificate(&client.leaf_der).unwrap();
        assert_eq!(
            cert.subject()
                .iter_common_name()
                .next()
                .unwrap()
                .as_str()
                .unwrap(),
            "fake-rdp"
        );
    })
    .await;
}

#[tokio::test]
async fn connections_counted() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid(), 1024, 768).await.unwrap();
        assert_eq!(server.connections(), 0);
        finalize_failure(
            ProbeClient::connect(server.addr(), config("alice", "wrong", None, true)).await,
        );
        let _client = ProbeClient::connect(server.addr(), config("alice", "pw-1", None, true))
            .await
            .unwrap();
        assert_eq!(server.connections(), 2);
    })
    .await;
}

#[tokio::test]
async fn start_with_customizes_once() {
    wall_clock(async {
        let called = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&called);
        struct Factory(Arc<AtomicUsize>);
        impl ironrdp_server::StaticChannelFactory for Factory {
            fn attach(&self, _: &mut ironrdp_server::Acceptor) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let attached = Arc::new(AtomicUsize::new(0));
        let factory = Factory(Arc::clone(&attached));
        let server = FakeRdpServer::start_with(FakeSecurity::Tls, 1024, 768, move |builder| {
            observed.fetch_add(1, Ordering::SeqCst);
            builder.with_static_channel_factory(Box::new(factory))
        })
        .await
        .unwrap();
        let _client = ProbeClient::connect(server.addr(), config("", "", None, false))
            .await
            .unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(attached.load(Ordering::SeqCst), 1);
    })
    .await;
}

#[tokio::test]
async fn pointer_updates_reach_client() {
    wall_clock(async {
        let server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let mut client = ProbeClient::connect(server.addr(), config("", "", None, false))
            .await
            .unwrap();
        let mut rgba = [12, 34, 56, 255].repeat(16 * 16);
        rgba[..4].copy_from_slice(&[90, 80, 70, 255]);
        rgba[15 * 16 * 4..15 * 16 * 4 + 4].copy_from_slice(&[1, 2, 3, 255]);
        server.push_pointer_bitmap(16, 16, 3, 4, rgba.clone());
        server.push_pointer_hidden();
        server.push_pointer_default();
        let (mut bitmap, mut hidden) = (false, false);
        loop {
            for output in tokio::time::timeout(Duration::from_secs(5), client.next())
                .await
                .expect("waiting for a pointer update")
                .unwrap()
            {
                match output {
                    ActiveStageOutput::PointerBitmap(pointer) => {
                        assert_eq!((pointer.width, pointer.height), (16, 16));
                        assert_eq!((pointer.hotspot_x, pointer.hotspot_y), (3, 4));
                        assert_eq!(pointer.bitmap_data, rgba);
                        bitmap = true;
                    }
                    ActiveStageOutput::PointerHidden if bitmap => hidden = true,
                    ActiveStageOutput::PointerDefault if bitmap && hidden => return,
                    _ => {}
                }
            }
        }
    })
    .await;
}

#[tokio::test]
async fn hybrid_domain_credentials() {
    wall_clock(async {
        let server = FakeRdpServer::start(
            FakeSecurity::Hybrid {
                user: "alice".into(),
                password: "pw-1".into(),
                domain: Some("EXAMPLE".into()),
            },
            1024,
            768,
        )
        .await
        .unwrap();
        finalize_failure(
            ProbeClient::connect(
                server.addr(),
                config("alice", "pw-1", Some("WRONG".into()), true),
            )
            .await,
        );
        let _client = ProbeClient::connect(
            server.addr(),
            config("alice", "pw-1", Some("EXAMPLE".into()), true),
        )
        .await
        .unwrap();
    })
    .await;
}

#[tokio::test]
async fn drop_stops_listener() {
    wall_clock(async {
        let server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let addr = server.addr();
        drop(server);
        let error = tokio::net::TcpStream::connect(addr).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionRefused);
    })
    .await;
}
