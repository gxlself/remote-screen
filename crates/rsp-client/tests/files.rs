#[allow(dead_code)]
mod support;

use std::{collections::HashSet, sync::Arc, time::Duration};

use prost::Message;
use rsp_client::{
    files::{FileCtl, FileReplies, FileReply, FILES_FEATURES},
    session::{EventStream, Session, SessionError, SessionEvent, SessionHandle, SessionParams},
    SessionStateKind,
};
use rsp_core::{features, pb};
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    Conn, ConnError, ControlStream, QuicListener, StreamKind, UniRecv,
};
use tokio::time::{timeout, Instant};

fn params(host: &support::Host) -> SessionParams {
    let mut params = SessionParams::single(
        host.addr,
        host.fingerprint,
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    params.intent = pb::Intent::Files;
    params.features = FILES_FEATURES;
    params.required_features = features::FILE_BROWSE;
    params
}

async fn next_event(
    events: &mut EventStream,
    matches: impl Fn(&SessionEvent) -> bool,
) -> SessionEvent {
    timeout(support::TEST_TIMEOUT, async {
        loop {
            let event = events.recv().await.expect("session event");
            if matches(&event) {
                return event;
            }
            assert!(
                !matches!(event, SessionEvent::Ended { .. }),
                "session ended before expected event"
            );
        }
    })
    .await
    .expect("event deadline")
}

async fn authenticate(control: &ControlStream, expected: (pb::Intent, u64, u64)) {
    let pb::envelope::Msg::ClientHello(hello) = support::recv(control).await else {
        panic!("expected ClientHello");
    };
    assert_eq!(hello.intent, expected.0 as i32);
    assert_eq!(hello.features, expected.1);
    assert_eq!(hello.required_features, expected.2);
    support::send(
        control,
        pb::ServerHello {
            protocol: features::PROTOCOL_VERSION,
            // Advertising feedback ensures a mistakenly scheduled Feedback is
            // observable even though FILES clients do not advertise it.
            features: FILES_FEATURES | features::RATE_FEEDBACK,
            ..Default::default()
        },
    )
    .await;
    support::send(
        control,
        pb::AuthResult {
            status: pb::auth_result::Status::Ok as i32,
            role: if expected.0 == pb::Intent::Files {
                pb::Role::Viewer
            } else {
                pb::Role::Controller
            } as i32,
            ..Default::default()
        },
    )
    .await;
}

async fn connect(
    host: &support::Host,
    params: SessionParams,
) -> (SessionHandle, EventStream, Conn, ControlStream) {
    let expected = (params.intent, params.features, params.required_features);
    let (session, (conn, control)) = tokio::join!(Session::open(params), host.accept());
    let (handle, mut events) = session.expect("session open");
    authenticate(&control, expected).await;
    next_event(&mut events, |event| {
        matches!(
            event,
            SessionEvent::StateChanged(SessionStateKind::Streaming)
        )
    })
    .await;
    (handle, events, conn, control)
}

async fn read_all(recv: &mut UniRecv) -> Vec<u8> {
    timeout(support::TEST_TIMEOUT, async {
        let mut data = Vec::new();
        let mut buf = [0; 64];
        while let Some(count) = recv.read_raw(&mut buf).await.expect("raw read") {
            data.extend_from_slice(&buf[..count]);
        }
        data
    })
    .await
    .expect("file read deadline")
}

#[tokio::test]
async fn files_handshake_keeps_ping_without_start_stream_or_feedback() {
    let host = support::Host::bind().await;
    let (handle, _events, conn, control) = connect(&host, params(&host)).await;
    let started = Instant::now();
    let mut first_ping = None;
    let observation = Duration::from_millis(450);
    while started.elapsed() < observation {
        match tokio::time::timeout_at(started + observation, support::recv(&control)).await {
            Ok(pb::envelope::Msg::StartStream(_)) => panic!("FILES sent StartStream"),
            Ok(pb::envelope::Msg::Feedback(_)) => panic!("FILES sent Feedback"),
            Ok(pb::envelope::Msg::Ping(ping)) => {
                first_ping.get_or_insert(started.elapsed());
                support::send(
                    &control,
                    pb::Pong {
                        t1_us: ping.t1_us,
                        t2_us: rsp_core::clock::unix_micros(),
                    },
                )
                .await;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    assert!(first_ping.expect("FILES Ping") < Duration::from_secs(1));
    assert!(handle.stats().pings_sent > 0);
    assert!(
        handle.stats().rtt_us.is_some(),
        "clock synchronization works"
    );
    support::finish(&conn, &control).await;
}

#[tokio::test]
async fn control_session_still_starts_video() {
    let host = support::Host::bind().await;
    let mut params = params(&host);
    params.intent = pb::Intent::Control;
    params.features = rsp_client::session::CLIENT_FEATURES;
    params.required_features = 0;
    let (_handle, _events, conn, control) = connect(&host, params).await;
    assert!(matches!(
        support::recv(&control).await,
        pb::envelope::Msg::StartStream(_)
    ));
    support::finish(&conn, &control).await;
}

#[tokio::test]
async fn persistent_cursor_stream_does_not_block_files_or_later_cursor_messages() {
    let host = support::Host::bind().await;
    let (_handle, mut events, conn, control) = connect(&host, params(&host)).await;
    let mut cursor = conn.open_uni(StreamKind::Cursor).await.expect("cursor");
    let first = pb::CursorMsg::default();
    cursor
        .send_record(&first.encode_to_vec())
        .await
        .expect("cursor record");
    next_event(&mut events, |event| {
        matches!(event, SessionEvent::CursorMessage(_))
    })
    .await;

    // Keep cursor open while transferring a separate raw file stream.
    let mut file = conn.open_uni(StreamKind::File).await.expect("file");
    file.write_raw(b"0123456789").await.expect("file bytes");
    file.finish().await.expect("file FIN");
    let SessionEvent::FileStream(mut incoming) = next_event(&mut events, |event| {
        matches!(event, SessionEvent::FileStream(_))
    })
    .await
    else {
        unreachable!();
    };
    assert_eq!(format!("{incoming:?}"), "IncomingFile");
    assert_eq!(read_all(&mut incoming.0).await, b"0123456789");

    cursor
        .send_record(&first.encode_to_vec())
        .await
        .expect("later cursor record");
    next_event(&mut events, |event| {
        matches!(event, SessionEvent::CursorMessage(_))
    })
    .await;
    support::finish(&conn, &control).await;
}

#[tokio::test]
async fn file_stream_without_transfer_feature_is_stopped_with_code_six() {
    let host = support::Host::bind().await;
    let mut params = params(&host);
    params.features &= !features::FILE_TRANSFER;
    let (_handle, mut events, conn, control) = connect(&host, params).await;
    // Use Quinn only to observe STOP_SENDING; UniSend deliberately hides it.
    let mut file = conn
        .as_quic()
        .expect("QUIC")
        .open_uni()
        .await
        .expect("file");
    file.write_all(&[StreamKind::File as u8])
        .await
        .expect("kind byte");
    let code = timeout(support::TEST_TIMEOUT, file.stopped())
        .await
        .expect("stop deadline")
        .expect("stop result")
        .expect("stop code");
    assert_eq!(code.into_inner(), 6);
    support::finish(&conn, &control).await;
    loop {
        match timeout(support::TEST_TIMEOUT, events.recv())
            .await
            .expect("event deadline")
            .expect("event")
        {
            SessionEvent::FileStream(_) => panic!("unadvertised file stream was delivered"),
            SessionEvent::Ended { .. } => break,
            _ => {}
        }
    }
}

#[tokio::test]
async fn client_opens_file_stream_and_writes_raw_bytes() {
    let host = support::Host::bind().await;
    let (handle, _events, conn, control) = connect(&host, params(&host)).await;
    let mut send = timeout(support::TEST_TIMEOUT, handle.open_file_stream())
        .await
        .expect("open deadline")
        .expect("upload stream");
    send.write_raw(b"upload bytes").await.expect("upload");
    send.finish().await.expect("upload FIN");
    let (kind, mut recv) = timeout(support::TEST_TIMEOUT, conn.accept_uni())
        .await
        .expect("accept deadline")
        .expect("upload accepted");
    assert_eq!(kind, StreamKind::File);
    assert_eq!(read_all(&mut recv).await, b"upload bytes");
    support::finish(&conn, &control).await;
}

#[tokio::test]
async fn all_five_file_control_messages_keep_their_fields() {
    let host = support::Host::bind().await;
    let (handle, _events, conn, control) = connect(&host, params(&host)).await;
    let list = pb::FileList {
        request_id: 7,
        path: "/files".into(),
        offset: 11,
    };
    let request = pb::FileRequest {
        request_id: 8,
        path: "/files/example.bin".into(),
        transfer_id: "transfer".into(),
        offset: 42,
        expect_size: 99,
        expect_mtime_us: 123,
        expect_sha256: vec![1; 32],
    };
    let query = pb::FileQuery {
        request_id: 9,
        transfer_id: "transfer".into(),
    };
    let status = pb::FileStatus {
        request_id: 10,
        transfer_id: "transfer".into(),
        state: pb::file_status::State::Completed as i32,
        received: 99,
        error: pb::FileError::None as i32,
        saved_name: "example.bin".into(),
    };
    let cancel = pb::FileCancel {
        transfer_id: "transfer".into(),
    };
    let messages = [
        (FileCtl::List(list.clone()), pb::Envelope::from(list)),
        (
            FileCtl::Request(request.clone()),
            pb::Envelope::from(request),
        ),
        (FileCtl::Query(query.clone()), pb::Envelope::from(query)),
        (FileCtl::Status(status.clone()), pb::Envelope::from(status)),
        (FileCtl::Cancel(cancel.clone()), pb::Envelope::from(cancel)),
    ];
    for (message, expected) in messages {
        handle.send_file_ctl(message);
        assert_eq!(Some(support::recv_command(&control).await), expected.msg);
    }
    support::finish(&conn, &control).await;
}

#[tokio::test]
async fn host_file_replies_and_cancel_are_routed_to_session_events() {
    let host = support::Host::bind().await;
    let (_handle, mut events, conn, control) = connect(&host, params(&host)).await;
    let replies = FileReplies::default();
    let listing = pb::FileListing {
        request_id: 7,
        path: "/files".into(),
        ..Default::default()
    };
    let waiting = replies.wait(7);
    support::send(&control, listing.clone()).await;
    let event = next_event(&mut events, |event| {
        matches!(event, SessionEvent::FileListing(_))
    })
    .await;
    assert!(replies.route(&event));
    let FileReply::Listing(actual) = waiting.await.expect("listing reply") else {
        panic!("expected Listing");
    };
    assert_eq!(actual, listing);
    assert!(!replies.route(&event), "reply is consumed once");

    let waiting = replies.wait(8);
    let status = pb::FileStatus {
        request_id: 8,
        received: 42,
        ..Default::default()
    };
    support::send(&control, status.clone()).await;
    let event = next_event(&mut events, |event| {
        matches!(event, SessionEvent::FileStatus(_))
    })
    .await;
    assert!(
        replies.clone().route(&event),
        "clones share pending replies"
    );
    let FileReply::Status(actual) = waiting.await.expect("status reply") else {
        panic!("expected Status");
    };
    assert_eq!(actual, status);

    let mut zero = replies.wait(0);
    support::send(&control, pb::FileStatus::default()).await;
    let event = next_event(&mut events, |event| {
        matches!(event, SessionEvent::FileStatus(_))
    })
    .await;
    assert!(
        !replies.route(&event),
        "id zero is an unsolicited notification"
    );
    assert!(matches!(
        zero.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    let cancel = pb::FileCancel {
        transfer_id: "transfer".into(),
    };
    support::send(&control, cancel.clone()).await;
    let event = next_event(&mut events, |event| {
        matches!(event, SessionEvent::FileCancel(_))
    })
    .await;
    let SessionEvent::FileCancel(actual) = &event else {
        unreachable!()
    };
    assert_eq!(actual, &cancel);
    assert!(!replies.route(&event));

    // Unexpected App-only requests must be ignored without ending the session.
    support::send(&control, pb::FileList::default()).await;
    support::send(&control, pb::FileRequest::default()).await;
    support::send(&control, pb::FileQuery::default()).await;
    support::send(
        &control,
        pb::FileListing {
            request_id: 99,
            ..Default::default()
        },
    )
    .await;
    let event = next_event(&mut events, |event| {
        matches!(event, SessionEvent::FileListing(_))
    })
    .await;
    assert!(!replies.route(&event), "unmatched reply stays with caller");
    support::finish(&conn, &control).await;
    next_event(&mut events, |event| {
        matches!(event, SessionEvent::Ended { .. })
    })
    .await;
    replies.fail_all();
    assert!(zero.await.is_err());
}

#[tokio::test]
async fn fail_all_immediately_cancels_every_pending_reply() {
    let replies = FileReplies::default();
    let first = replies.wait(1);
    let second = replies.wait(2);
    replies.clone().fail_all();
    assert!(timeout(Duration::from_secs(1), first)
        .await
        .expect("first waiter deadline")
        .is_err());
    assert!(timeout(Duration::from_secs(1), second)
        .await
        .expect("second waiter deadline")
        .is_err());
}

#[test]
fn next_ids_start_at_one_and_are_unique_across_clones_and_threads() {
    let replies = FileReplies::default();
    assert_eq!(replies.next_id(), 1);
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let replies = replies.clone();
            std::thread::spawn(move || (0..1024).map(|_| replies.next_id()).collect::<Vec<_>>())
        })
        .collect();
    let ids: HashSet<_> = workers
        .into_iter()
        .flat_map(|worker| worker.join().expect("id allocator"))
        .collect();
    assert_eq!(ids.len(), 4096);
    assert!(!ids.contains(&0));
    assert!(!ids.contains(&1));
}

#[tokio::test]
async fn open_file_stream_after_session_end_returns_closed_without_hanging() {
    let host = support::Host::bind().await;
    let (handle, mut events, conn, control) = connect(&host, params(&host)).await;
    support::finish(&conn, &control).await;
    next_event(&mut events, |event| {
        matches!(event, SessionEvent::Ended { .. })
    })
    .await;
    assert!(matches!(
        timeout(Duration::from_secs(1), handle.open_file_stream())
            .await
            .expect("closed open deadline"),
        Err(SessionError::Transport(ConnError::Closed))
    ));
}

#[tokio::test]
async fn stream_credit_wait_does_not_block_ping_or_shutdown() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("host identity");
    let tls = rsp_transport::tls::server_config(&identity).expect("TLS");
    let listener = QuicListener::bind("127.0.0.1:0".parse().expect("loopback"), tls.clone())
        .await
        .expect("listener");
    let crypto =
        quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(tls)).expect("QUIC TLS");
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_uni_streams(0_u32.into());
    config.transport_config(Arc::new(transport));
    listener.endpoint().set_server_config(Some(config));
    let host = support::Host {
        addr: listener.local_addr().expect("address"),
        fingerprint: identity.fingerprint(),
        listener,
    };
    let (handle, mut events, conn, control) = connect(&host, params(&host)).await;
    let opening = handle.open_file_stream();
    tokio::pin!(opening);
    assert!(
        timeout(Duration::from_millis(100), &mut opening)
            .await
            .is_err(),
        "opening waits for stream credit"
    );
    assert!(matches!(
        timeout(Duration::from_secs(1), support::recv(&control))
            .await
            .expect("Ping while opening"),
        pb::envelope::Msg::Ping(_)
    ));
    support::finish(&conn, &control).await;
    assert!(timeout(Duration::from_secs(1), opening)
        .await
        .expect("pending open terminates")
        .is_err());
    next_event(&mut events, |event| {
        matches!(event, SessionEvent::Ended { .. })
    })
    .await;
}

#[tokio::test]
async fn tcp_file_streams_upload_and_download_without_ending_session() {
    let identity = LocalIdentity::generate_software(CertKind::Host).expect("host identity");
    let listener = rsp_transport::TcpListener::bind(
        "127.0.0.1:0".parse().expect("loopback"),
        rsp_transport::tls::server_config(&identity).expect("TLS"),
    )
    .await
    .expect("TCP listener");
    let mut params = SessionParams::single(
        listener.local_addr().expect("address"),
        identity.fingerprint(),
        LocalIdentity::generate_software(CertKind::App).expect("app identity"),
    );
    params.intent = pb::Intent::Files;
    params.features = FILES_FEATURES;
    params.required_features = features::FILE_BROWSE;
    let expected = (params.intent, params.features, params.required_features);
    let (session, (conn, control)) = tokio::join!(Session::open(params), async {
        let conn = listener
            .accept()
            .await
            .expect("incoming")
            .accept()
            .await
            .expect("TLS")
            .expect("main connection");
        let control = conn.accept_control().await.expect("control");
        (conn, control)
    });
    let (handle, mut events) = session.expect("TCP session");
    authenticate(&control, expected).await;
    next_event(&mut events, |event| {
        matches!(
            event,
            SessionEvent::StateChanged(SessionStateKind::Streaming)
        )
    })
    .await;

    let mut upload = timeout(Duration::from_secs(1), handle.open_file_stream())
        .await
        .expect("TCP upload open deadline")
        .expect("TCP upload stream");
    upload.write_raw(b"tcp upload").await.expect("upload bytes");
    upload.finish().await.expect("upload FIN");
    let (kind, mut received) = timeout(Duration::from_secs(1), conn.accept_uni())
        .await
        .expect("TCP upload accept deadline")
        .expect("TCP upload accepted");
    assert_eq!(kind, StreamKind::File);
    assert_eq!(read_all(&mut received).await, b"tcp upload");

    let mut download = conn.open_uni(StreamKind::File).await.expect("download");
    download
        .write_raw(b"tcp download")
        .await
        .expect("download bytes");
    download.finish().await.expect("download FIN");
    let SessionEvent::FileStream(mut incoming) = next_event(&mut events, |event| {
        matches!(event, SessionEvent::FileStream(_))
    })
    .await
    else {
        unreachable!();
    };
    assert_eq!(read_all(&mut incoming.0).await, b"tcp download");

    handle.send_file_ctl(FileCtl::List(pb::FileList::default()));
    assert!(matches!(
        support::recv_command(&control).await,
        pb::envelope::Msg::FileList(_)
    ));
    support::finish(&conn, &control).await;
}
