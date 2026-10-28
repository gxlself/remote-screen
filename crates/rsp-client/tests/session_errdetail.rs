#[allow(dead_code)]
mod support;

use std::{collections::BTreeMap, time::Duration};

use rsp_client::{
    EventStream, Session, SessionError, SessionEvent, SessionParams, SessionStateKind,
};
use rsp_core::{errors::ErrorCode, features, ids::fp_b64url, pb};
use rsp_transport::{
    dial::{IfaceInfo, PathKind},
    identity::{CertKind, LocalIdentity},
};
use tokio::time::timeout;

fn params(host: &support::Host) -> SessionParams {
    SessionParams::single(
        host.addr,
        host.fingerprint,
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    )
}

async fn through_end(events: &mut EventStream) -> Vec<SessionEvent> {
    timeout(support::TEST_TIMEOUT, async {
        let mut received = Vec::new();
        loop {
            let event = events
                .recv()
                .await
                .expect("session ends with a typed event");
            let ended = matches!(event, SessionEvent::Ended { .. });
            received.push(event);
            if ended {
                return received;
            }
        }
    })
    .await
    .expect("session deadline")
}

fn assert_error_pair(
    events: &[SessionEvent],
    expected_code: ErrorCode,
    expected_detail: &BTreeMap<String, String>,
    expected_retry: Option<u32>,
) {
    let index = events
        .iter()
        .position(
            |event| matches!(event, SessionEvent::Error { code, .. } if *code == expected_code),
        )
        .expect("Error event");
    assert!(index > 0, "ErrorDetail immediately precedes Error");
    match &events[index - 1] {
        SessionEvent::ErrorDetail {
            code,
            detail,
            retry_after_s,
        } => {
            assert_eq!(*code, expected_code);
            assert_eq!(detail, expected_detail);
            assert_eq!(*retry_after_s, expected_retry);
        }
        event => panic!("expected adjacent ErrorDetail, got {event:?}"),
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::ErrorDetail { .. }))
            .count(),
        1
    );
}

async fn denied(detail: BTreeMap<String, String>, retry_after_s: u32) -> Vec<SessionEvent> {
    let host = support::Host::bind().await;
    let params = params(&host);
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        // Policy denials can precede ServerHello.
        assert!(matches!(
            support::recv(&control).await,
            pb::envelope::Msg::ClientHello(_)
        ));
        support::send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Denied as i32,
                error: Some(pb::ErrorInfo {
                    code: 140,
                    message: "approval denied".into(),
                    detail: detail.into_iter().collect(),
                    retry_after_s,
                }),
                ..Default::default()
            },
        )
        .await;
        conn.closed().await;
    });
    let (_handle, mut events) = Session::open(params).await.expect("open");
    let events = through_end(&mut events).await;
    server.await.expect("Host denial script");
    events
}

#[tokio::test]
async fn host_denial_detail_immediately_precedes_error() {
    let detail = BTreeMap::from([("reason".into(), "timeout".into())]);
    let events = denied(detail.clone(), 0).await;
    assert_error_pair(&events, ErrorCode::ApprovalDenied, &detail, None);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::Error { code: ErrorCode::ApprovalDenied, message } if message == "approval denied"
    )));
}

#[tokio::test]
async fn host_denial_without_context_only_emits_error() {
    let events = denied(BTreeMap::new(), 0).await;
    assert!(!events
        .iter()
        .any(|event| matches!(event, SessionEvent::ErrorDetail { .. })));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                SessionEvent::Error {
                    code: ErrorCode::ApprovalDenied,
                    ..
                }
            ))
            .count(),
        1
    );
}

#[tokio::test]
async fn host_denial_preserves_retry_with_or_without_detail() {
    for detail in [
        BTreeMap::new(),
        BTreeMap::from([("retries_left".into(), "2".into())]),
    ] {
        let events = denied(detail.clone(), 17).await;
        assert_error_pair(&events, ErrorCode::ApprovalDenied, &detail, Some(17));
    }
}

#[tokio::test]
async fn control_error_preserves_detail_and_retry_without_ending_session() {
    for (detail, retry) in [
        (BTreeMap::from([("retries_left".into(), "1".into())]), 9),
        (BTreeMap::new(), 4),
        (BTreeMap::new(), 0),
    ] {
        let host = support::Host::bind().await;
        let params = params(&host);
        let host_detail = detail.clone();
        let server = tokio::spawn(async move {
            let (conn, control) = host.accept().await;
            support::hello(&control).await;
            support::auth(&control, pb::auth_result::Status::Ok).await;
            assert!(matches!(
                support::recv_command(&control).await,
                pb::envelope::Msg::StartStream(_)
            ));
            support::send(
                &control,
                pb::ErrorInfo {
                    code: 141,
                    message: "takeover denied".into(),
                    detail: host_detail.into_iter().collect(),
                    retry_after_s: retry,
                },
            )
            .await;
            // A command after the error proves the control flow stays usable.
            assert!(matches!(
                support::recv_command(&control).await,
                pb::envelope::Msg::RequestControl(_)
            ));
            support::finish(&conn, &control).await;
        });
        let (handle, mut stream) = Session::open(params).await.expect("open");
        let events = timeout(support::TEST_TIMEOUT, async {
            let mut events = Vec::new();
            loop {
                let event = stream.recv().await.expect("session event");
                if matches!(event, SessionEvent::Error { .. }) {
                    handle.request_control();
                }
                let ended = matches!(event, SessionEvent::Ended { .. });
                events.push(event);
                if ended {
                    break events;
                }
            }
        })
        .await
        .expect("control error deadline");
        if retry != 0 {
            assert_error_pair(&events, ErrorCode::TakeoverDenied, &detail, Some(retry));
        } else {
            assert!(!events
                .iter()
                .any(|event| matches!(event, SessionEvent::ErrorDetail { .. })));
        }
        server.await.expect("Host control script");
    }
}

#[tokio::test]
async fn local_approval_timeout_includes_timeout_reason() {
    let host = support::Host::bind().await;
    let params = params(&host);
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::PendingApproval).await;
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::Goodbye(_)
        ));
        conn.close(rsp_transport::CloseCode::Normal, b"timeout accepted");
    });
    let (handle, mut stream) = Session::open(params).await.expect("open");
    timeout(support::TEST_TIMEOUT, async {
        loop {
            if matches!(
                stream.recv().await.expect("pending approval event"),
                SessionEvent::StateChanged(SessionStateKind::PendingApproval)
            ) {
                break;
            }
        }
    })
    .await
    .expect("pending approval deadline");
    tokio::time::pause();
    let events = support::without_time_advancing(async {
        tokio::time::advance(Duration::from_secs(6)).await;
        through_end(&mut stream).await
    })
    .await;
    tokio::time::resume();
    assert_error_pair(
        &events,
        ErrorCode::ApprovalDenied,
        &BTreeMap::from([("reason".into(), "timeout".into())]),
        None,
    );
    assert_eq!(handle.state(), SessionStateKind::Ended);
    server.await.expect("Host timeout script");
}

#[tokio::test]
async fn fingerprint_mismatch_includes_received_pin_ip_and_original_path() {
    for (kind, path) in [
        (PathKind::Tailnet, "tailnet"),
        (PathKind::Intranet, "intranet"),
        (PathKind::Lan, "lan"),
        (PathKind::Manual, "manual"),
        (PathKind::LastGood(Box::new(PathKind::Lan)), "lan"),
    ] {
        let host = support::Host::bind().await;
        let expected = BTreeMap::from([
            ("fp".into(), fp_b64url(&host.fingerprint)),
            ("addr".into(), host.addr.ip().to_string()),
            ("path".into(), path.into()),
        ]);
        let mut params = params(&host);
        params.host_fingerprint = [0; 32];
        params.candidates[0].kind = kind;
        params.local_net.interfaces.push(IfaceInfo {
            name: "loopback".into(),
            addrs: vec!["127.0.0.0/8".parse().expect("loopback subnet")],
            is_vpn: false,
        });
        let server = tokio::spawn(async move {
            let incoming = host.listener.accept().await.expect("incoming");
            if let Ok(conn) = incoming.accept().await {
                assert!(conn.accept_control().await.is_err());
            }
        });
        match timeout(support::TEST_TIMEOUT, Session::open(params))
            .await
            .expect("dial deadline")
        {
            Err(SessionError::Dial { code, detail }) => {
                assert_eq!(code, ErrorCode::FingerprintMismatch);
                assert_eq!(detail, expected);
            }
            _ => panic!("expected fingerprint mismatch"),
        }
        server.await.expect("Host pin script");
    }
}

#[tokio::test]
async fn local_version_errors_include_host_version() {
    for (protocol, required, server_features, client_required, code) in [
        (0, 0, 0, 0, ErrorCode::HostTooOld),
        (2, 0, 0, 0, ErrorCode::AppTooOld),
        (1, 1u64 << 63, 0, 0, ErrorCode::AppTooOld),
        (1, 0, 0, features::FEC_RS, ErrorCode::HostTooOld),
    ] {
        let host = support::Host::bind().await;
        let mut params = params(&host);
        params.required_features = client_required;
        let server = tokio::spawn(async move {
            let (conn, control) = host.accept().await;
            let _ = support::recv(&control).await;
            support::send(
                &control,
                pb::ServerHello {
                    protocol,
                    host_version: "0.2.4".into(),
                    required_features: required,
                    features: server_features,
                    ..Default::default()
                },
            )
            .await;
            conn.closed().await;
        });
        let (_handle, mut events) = Session::open(params).await.expect("open");
        let events = through_end(&mut events).await;
        assert_error_pair(
            &events,
            code,
            &BTreeMap::from([("host_version".into(), "0.2.4".into())]),
            None,
        );
        server.await.expect("Host version script");
    }
}

#[tokio::test]
async fn client_hello_advertises_hq_refresh() {
    let host = support::Host::bind().await;
    let params = params(&host);
    let server = tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        let pb::envelope::Msg::ClientHello(hello) = support::recv(&control).await else {
            panic!("ClientHello");
        };
        assert_ne!(hello.features & features::HQ_REFRESH, 0);
        support::send(
            &control,
            pb::AuthResult {
                status: pb::auth_result::Status::Denied as i32,
                ..Default::default()
            },
        )
        .await;
        conn.closed().await;
    });
    let (_handle, mut events) = Session::open(params).await.expect("open");
    let _ = through_end(&mut events).await;
    server.await.expect("Host feature script");
}
