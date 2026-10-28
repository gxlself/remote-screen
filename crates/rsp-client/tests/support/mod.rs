//! Scripted, authenticated loopback Host for session protocol tests.

use std::{future::Future, time::Duration};

use prost::Message;
use rsp_core::pb;
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    tls::server_config,
    CloseCode, Conn, ControlStream, QuicListener,
};
use tokio::sync::oneshot;
use tokio::time::timeout;

pub const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Poll real loopback I/O without letting Tokio auto-advance a paused clock.
/// Only explicit `advance` calls in the future may move session timers. The
/// wall-clock deadline still catches stalled I/O when Tokio time is frozen.
pub async fn without_time_advancing<T>(future: impl Future<Output = T>) -> T {
    let (deadline_tx, deadline_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
    // Tokio inhibits paused-clock auto-advance while a blocking task runs.
    // Keep this task alive for the entire I/O wait, then wake it to exit.
    let watchdog = tokio::task::spawn_blocking(move || {
        if cancel_rx.recv_timeout(TEST_TIMEOUT).is_err() {
            let _ = deadline_tx.send(());
        }
    });
    tokio::pin!(future);
    tokio::select! {
        biased;
        result = &mut future => {
            let _ = cancel_tx.send(());
            let _ = watchdog.await;
            result
        }
        _ = deadline_rx => {
            let _ = watchdog.await;
            panic!("paused-time I/O deadline");
        }
    }
}

pub struct Host {
    pub addr: std::net::SocketAddr,
    pub fingerprint: [u8; 32],
    pub listener: QuicListener,
}

impl Host {
    pub async fn bind() -> Self {
        let identity = LocalIdentity::generate_software(CertKind::Host).expect("host identity");
        let listener = QuicListener::bind(
            "127.0.0.1:0".parse().expect("loopback"),
            server_config(&identity).expect("TLS config"),
        )
        .await
        .expect("Host listener");
        Self {
            addr: listener.local_addr().expect("bound address"),
            fingerprint: identity.fingerprint(),
            listener,
        }
    }

    pub async fn accept(&self) -> (Conn, ControlStream) {
        let conn = timeout(TEST_TIMEOUT, async {
            self.listener
                .accept()
                .await
                .expect("incoming")
                .accept()
                .await
        })
        .await
        .expect("handshake deadline")
        .expect("mutual TLS handshake");
        let control = conn.accept_control().await.expect("control stream");
        (conn, control)
    }
}

pub async fn recv(control: &ControlStream) -> pb::envelope::Msg {
    let payload = timeout(TEST_TIMEOUT, control.recv())
        .await
        .expect("control deadline")
        .expect("control read")
        .expect("control record");
    pb::Envelope::decode(payload)
        .expect("protobuf")
        .msg
        .expect("message")
}

/// Periodic Pings may precede a command if the executor is busy. Reply as a
/// real Host would, while keeping command ordering and a total deadline.
pub async fn recv_command(control: &ControlStream) -> pb::envelope::Msg {
    timeout(TEST_TIMEOUT, async {
        loop {
            match recv(control).await {
                pb::envelope::Msg::Ping(ping) => {
                    send(
                        control,
                        pb::Pong {
                            t1_us: ping.t1_us,
                            t2_us: rsp_core::clock::unix_micros(),
                        },
                    )
                    .await;
                }
                command => return command,
            }
        }
    })
    .await
    .expect("command deadline")
}

pub async fn send(control: &ControlStream, message: impl Into<pb::Envelope>) {
    let framed = rsp_core::control::encode(&message.into());
    // The A1 codec includes its length prefix; Conn's ControlStream owns that
    // framing on the wire, so pass only the encoded envelope here.
    control.send(&framed[4..]).await.expect("control write");
}

pub async fn hello(control: &ControlStream) {
    assert!(matches!(
        recv(control).await,
        pb::envelope::Msg::ClientHello(_)
    ));
    send(
        control,
        pb::ServerHello {
            protocol: 1,
            host_name: "scripted-host".to_owned(),
            ..Default::default()
        },
    )
    .await;
}

pub async fn auth(control: &ControlStream, status: pb::auth_result::Status) {
    send(
        control,
        pb::AuthResult {
            status: status as i32,
            role: pb::Role::Controller as i32,
            session_token: vec![7; 16],
            approval_timeout_s: 1,
            ..Default::default()
        },
    )
    .await;
}

pub async fn finish(conn: &Conn, control: &ControlStream) {
    send(
        control,
        pb::Goodbye {
            reason: pb::goodbye::Reason::UserClosed as i32,
            message: String::new(),
        },
    )
    .await;
    let _ = control.finish().await;
    timeout(TEST_TIMEOUT, conn.closed())
        .await
        .expect("peer receives Goodbye and closes");
    conn.close(CloseCode::Normal, b"script finished");
}
