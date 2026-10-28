#[allow(dead_code)]
mod support;

use rsp_client::session::{Session, SessionHandle, SessionParams, SessionStateKind};
use rsp_client::{EventStream, SessionEvent};
use rsp_core::{features, pb};
use rsp_transport::identity::{CertKind, LocalIdentity};
use rsp_transport::{Conn, ControlStream};
use tokio::time::timeout;

struct Streaming {
    conn: Conn,
    control: ControlStream,
    handle: SessionHandle,
    events: EventStream,
}

impl Streaming {
    async fn open(host: &support::Host, server_features: u64) -> Self {
        let params = SessionParams::single(
            host.addr,
            host.fingerprint,
            LocalIdentity::generate_software(CertKind::App).expect("App identity"),
        );
        let (client, (conn, control)) = tokio::join!(Session::open(params), host.accept());
        let (handle, mut events) = client.expect("open session");
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::ClientHello(_)
        ));
        support::send(
            &control,
            pb::ServerHello {
                protocol: features::PROTOCOL_VERSION,
                features: server_features,
                ..Default::default()
            },
        )
        .await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        timeout(support::TEST_TIMEOUT, async {
            while !matches!(
                events.recv().await.expect("session event"),
                SessionEvent::StateChanged(SessionStateKind::Streaming)
            ) {}
        })
        .await
        .expect("streaming deadline");
        Self {
            conn,
            control,
            handle,
            events,
        }
    }

    async fn finish(mut self) {
        support::finish(&self.conn, &self.control).await;
        timeout(support::TEST_TIMEOUT, async {
            while !matches!(
                self.events.recv().await.expect("session end"),
                SessionEvent::Ended { code: None, .. }
            ) {}
        })
        .await
        .expect("session end deadline");
    }
}

#[tokio::test]
async fn set_recording_sends_notice_when_host_advertises_recording_notice() {
    let host = support::Host::bind().await;
    let session = Streaming::open(&host, features::RECORDING_NOTICE).await;
    for active in [true, false] {
        session.handle.set_recording(active);
        assert!(matches!(
            support::recv_command(&session.control).await,
            pb::envelope::Msg::RecordingNotice(notice) if notice.active == active
        ));
    }
    session.finish().await;
}

#[tokio::test]
async fn set_recording_is_silent_for_hosts_without_recording_notice() {
    let host = support::Host::bind().await;
    let session = Streaming::open(&host, 0).await;
    session.handle.set_recording(true);
    session.handle.set_recording(false);
    // Commands run in order, so a notice would arrive before this PauseStream.
    session.handle.pause();
    assert!(matches!(
        support::recv_command(&session.control).await,
        pb::envelope::Msg::PauseStream(_)
    ));
    assert_eq!(session.handle.state(), SessionStateKind::Streaming);
    session.finish().await;
}
