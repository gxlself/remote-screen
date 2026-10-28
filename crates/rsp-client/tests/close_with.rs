#[allow(dead_code)]
mod support;

use rsp_client::{Session, SessionEvent, SessionStateKind};
use rsp_core::pb;
use rsp_transport::identity::{CertKind, LocalIdentity};

fn session_params(host: &support::Host) -> rsp_client::SessionParams {
    rsp_client::SessionParams::single(
        host.addr,
        host.fingerprint,
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    )
}

#[tokio::test]
async fn close_with_background_sends_reason_and_typed_end() {
    let host = support::Host::bind().await;
    let params = session_params(&host);
    let server = tokio::spawn(async move {
        let (_conn, control) = host.accept().await;
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::Goodbye(goodbye)
                if goodbye.reason == pb::goodbye::Reason::Background as i32
        ));
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        support::without_time_advancing(events.recv()).await,
        Some(SessionEvent::StateChanged(SessionStateKind::Streaming))
    ) {}
    handle.close_with(pb::goodbye::Reason::Background);
    let mut ended = None;
    while let Some(event) = support::without_time_advancing(events.recv()).await {
        if let SessionEvent::Ended { kind, .. } = event {
            ended = Some(kind);
            break;
        }
    }
    assert_eq!(
        ended,
        Some(rsp_client::EndReason::Goodbye(
            pb::goodbye::Reason::Background
        ))
    );
    server.await.expect("Host script");
}

#[tokio::test]
async fn resumed_and_resume_token_are_visible_on_the_handle() {
    let host = support::Host::bind().await;
    let params = session_params(&host);
    let server = tokio::spawn(async move {
        let (_conn, control) = host.accept().await;
        support::hello(&control).await;
        support::send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Ok as i32,
                role: pb::Role::Controller as i32,
                resumed: true,
                session_token: vec![9; 16],
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        support::without_time_advancing(events.recv()).await,
        Some(SessionEvent::StateChanged(SessionStateKind::Streaming))
    ) {}
    assert!(handle.resumed());
    assert_eq!(handle.resume_token(), Some(vec![9; 16]));
    handle.close();
    server.await.expect("Host script");

    let host = support::Host::bind().await;
    let params = session_params(&host);
    let server = tokio::spawn(async move {
        let (_conn, control) = host.accept().await;
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
    });
    let (handle, mut events) = Session::open(params).await.expect("open");
    while !matches!(
        support::without_time_advancing(events.recv()).await,
        Some(SessionEvent::StateChanged(SessionStateKind::Streaming))
    ) {}
    assert!(!handle.resumed());
    handle.close();
    server.await.expect("Host script");
}
