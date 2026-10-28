use std::{future::Future, sync::Arc, time::Duration};

use bytes::Bytes;
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    mux::{
        file_chunk, FILE_CHANNEL, FILE_FLAG_FIN, FILE_FLAG_RESET, FILE_FLAG_STOP, FILE_TCP_CHUNK,
    },
    tcp::{MuxEvent, TcpMuxState, TcpRecordDecoder, TcpRecordHeader},
    tls::{client_config, server_config, ServerPin},
    CloseCode, Conn, ConnError, ControlStream, StreamKind, TcpListener, UniRecv, UniSend,
};
use tokio::{io::AsyncWriteExt, net::TcpStream};
use tokio_rustls::{client::TlsStream, TlsConnector};

const MIB: usize = 1 << 20;

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("TCP file test timed out")
}

async fn listener() -> (TcpListener, Arc<rustls::ClientConfig>) {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let cfg = client_config(&app, ServerPin::Fingerprint(host.fingerprint())).unwrap();
    let listener = TcpListener::bind(
        "127.0.0.1:0".parse().unwrap(),
        server_config(&host).unwrap(),
    )
    .await
    .unwrap();
    (listener, Arc::new(cfg))
}

struct Pair {
    client: Conn,
    server: Conn,
    client_control: ControlStream,
    server_control: ControlStream,
}

impl Drop for Pair {
    fn drop(&mut self) {
        self.client.close(CloseCode::Normal, b"test done");
        self.server.close(CloseCode::Normal, b"test done");
    }
}

async fn pair() -> Pair {
    let (listener, cfg) = listener().await;
    let ((client, client_control), server) = bounded(async {
        tokio::join!(
            async {
                let client = rsp_transport::connect_tcp(listener.local_addr().unwrap(), cfg)
                    .await
                    .unwrap();
                let control = client.open_control().await.unwrap();
                control.send(b"ready").await.unwrap();
                (client, control)
            },
            async {
                listener
                    .accept()
                    .await
                    .unwrap()
                    .accept()
                    .await
                    .unwrap()
                    .unwrap()
            }
        )
    })
    .await;
    let server_control = server.accept_control().await.unwrap();
    assert_eq!(server_control.recv().await.unwrap().unwrap(), b"ready"[..]);
    Pair {
        client,
        server,
        client_control,
        server_control,
    }
}

async fn raw_pair() -> (TlsStream<TcpStream>, Conn) {
    let (listener, cfg) = listener().await;
    bounded(async {
        tokio::join!(
            async {
                let socket = TcpStream::connect(listener.local_addr().unwrap())
                    .await
                    .unwrap();
                let mut tls = TlsConnector::from(cfg)
                    .connect(
                        rustls::pki_types::ServerName::try_from("rs-host").unwrap(),
                        socket,
                    )
                    .await
                    .unwrap();
                tls.write_all(&rsp_transport::encode_tcp_record(0, b"ready").unwrap())
                    .await
                    .unwrap();
                tls
            },
            async {
                listener
                    .accept()
                    .await
                    .unwrap()
                    .accept()
                    .await
                    .unwrap()
                    .unwrap()
            }
        )
    })
    .await
}

async fn accept(conn: &Conn, kind: StreamKind) -> UniRecv {
    let (received, stream) = bounded(conn.accept_uni()).await.unwrap();
    assert_eq!(received, kind);
    stream
}

async fn collect(stream: &mut UniRecv) -> Result<Vec<u8>, ConnError> {
    let mut out = Vec::new();
    let mut buf = [0; 7001];
    while let Some(n) = stream.read_raw(&mut buf).await? {
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

#[tokio::test]
async fn file_chunks_one_mib_and_fin() {
    let pair = pair().await;
    let body: Vec<_> = (0..MIB).map(|i| (i % 251) as u8).collect();
    let mut send = pair.client.open_uni(StreamKind::File).await.unwrap();
    bounded(async {
        tokio::join!(
            async {
                send.write_raw(&body).await.unwrap();
                send.finish().await.unwrap();
                assert!(matches!(
                    send.write_raw(b"late").await,
                    Err(ConnError::Closed)
                ));
            },
            async {
                let mut recv = accept(&pair.server, StreamKind::File).await;
                assert_eq!(collect(&mut recv).await.unwrap(), body);
                assert_eq!(recv.read_raw(&mut [0; 1]).await.unwrap(), None);
            }
        );
    })
    .await;
}

#[tokio::test]
async fn interleaved_files_do_not_mix_data() {
    let pair = pair().await;
    let mut a = pair.client.open_uni(StreamKind::File).await.unwrap();
    let mut b = pair.client.open_uni(StreamKind::File).await.unwrap();
    bounded(async {
        tokio::join!(
            async {
                for _ in 0..32 {
                    a.write_raw(&[b'a'; 10_000]).await.unwrap();
                    b.write_raw(&[b'b'; 13_000]).await.unwrap();
                }
                b.finish().await.unwrap();
                a.finish().await.unwrap();
            },
            async {
                let mut a = accept(&pair.server, StreamKind::File).await;
                let mut b = accept(&pair.server, StreamKind::File).await;
                let (a, b) = tokio::join!(collect(&mut a), collect(&mut b));
                assert_eq!(a.unwrap(), vec![b'a'; 320_000]);
                assert_eq!(b.unwrap(), vec![b'b'; 416_000]);
            }
        );
    })
    .await;
}

#[tokio::test]
async fn reset_is_a_stream_error_and_clipboard_survives() {
    let pair = pair().await;
    let mut file = pair.client.open_uni(StreamKind::File).await.unwrap();
    file.write_raw(b"partial").await.unwrap();
    file.reset(2).unwrap();
    assert!(matches!(
        file.write_raw(b"late").await,
        Err(ConnError::Closed)
    ));
    let mut recv = accept(&pair.server, StreamKind::File).await;
    assert!(
        matches!(bounded(collect(&mut recv)).await, Err(ConnError::Protocol(reason)) if reason.contains("reset (2)"))
    );

    let mut clipboard = pair.client.open_uni(StreamKind::Clipboard).await.unwrap();
    // Clipboard reset remains a no-op.
    clipboard.reset(2).unwrap();
    clipboard.send_record(b"clipboard survives").await.unwrap();
    clipboard.finish().await.unwrap();
    let mut recv = accept(&pair.server, StreamKind::Clipboard).await;
    assert_eq!(
        bounded(recv.recv_record()).await.unwrap().unwrap(),
        b"clipboard survives"[..]
    );
    assert_eq!(bounded(recv.recv_record()).await.unwrap(), None);
    pair.client_control.send(b"still connected").await.unwrap();
    assert_eq!(
        bounded(pair.server_control.recv()).await.unwrap().unwrap(),
        b"still connected"[..]
    );
}

#[tokio::test]
async fn reset_preserves_control_and_another_active_file_in_both_directions() {
    let pair = pair().await;
    for (sender, receiver, control_tx, control_rx) in [
        (
            &pair.client,
            &pair.server,
            &pair.client_control,
            &pair.server_control,
        ),
        (
            &pair.server,
            &pair.client,
            &pair.server_control,
            &pair.client_control,
        ),
    ] {
        let mut reset = sender.open_uni(StreamKind::File).await.unwrap();
        let mut survivor = sender.open_uni(StreamKind::File).await.unwrap();
        reset.write_raw(b"incomplete file").await.unwrap();
        survivor.write_raw(b"before reset; ").await.unwrap();
        let mut reset_rx = accept(receiver, StreamKind::File).await;
        let mut survivor_rx = accept(receiver, StreamKind::File).await;
        reset.reset(2).unwrap();
        match bounded(collect(&mut reset_rx)).await {
            Err(ConnError::Protocol(message)) => assert_eq!(message, "file stream reset (2)"),
            other => panic!("expected a stream reset, got {other:?}"),
        }
        control_tx.send(b"ping after reset").await.unwrap();
        assert_eq!(
            bounded(control_rx.recv()).await.unwrap().unwrap(),
            b"ping after reset"[..]
        );
        control_rx.send(b"pong after reset").await.unwrap();
        assert_eq!(
            bounded(control_tx.recv()).await.unwrap().unwrap(),
            b"pong after reset"[..]
        );
        survivor.write_raw(b"after reset").await.unwrap();
        survivor.finish().await.unwrap();
        assert_eq!(
            bounded(collect(&mut survivor_rx)).await.unwrap(),
            b"before reset; after reset"
        );
        assert_eq!(
            bounded(survivor_rx.read_raw(&mut [0; 1])).await.unwrap(),
            None
        );
    }
}

async fn write_until_stopped(send: &mut UniSend) -> usize {
    bounded(async {
        for count in 0..4096 {
            match send.write_raw(&[0x5a; FILE_TCP_CHUNK]).await {
                Ok(()) => tokio::task::yield_now().await,
                Err(ConnError::Closed) => return count,
                Err(error) => panic!("unexpected send error: {error}"),
            }
        }
        panic!("sender did not observe STOP within 64 MiB")
    })
    .await
}

#[tokio::test]
async fn stop_after_100_kib_discards_queued_and_late_data() {
    let pair = pair().await;
    let mut send = pair.client.open_uni(StreamKind::File).await.unwrap();
    bounded(async {
        tokio::join!(
            async {
                let chunks = write_until_stopped(&mut send).await;
                assert!(chunks >= 7);
                assert!(matches!(
                    send.write_raw(b"late").await,
                    Err(ConnError::Closed)
                ));
            },
            async {
                let mut recv = accept(&pair.server, StreamKind::File).await;
                let mut received = 0;
                let mut buf = [0; 4096];
                while received < 100 * 1024 {
                    received += recv.read_raw(&mut buf).await.unwrap().unwrap();
                }
                assert_eq!(received, 100 * 1024);
                recv.stop(3).unwrap();
                assert_eq!(recv.read_raw(&mut buf).await.unwrap(), None);
                // The reader must continue processing the connection even
                // while already queued chunks for this slot arrive.
                pair.client_control.send(b"after stop").await.unwrap();
                assert_eq!(
                    pair.server_control.recv().await.unwrap().unwrap(),
                    b"after stop"[..]
                );
            }
        );
    })
    .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), pair.server.accept_uni())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn bidirectional_files_fin_and_stop_do_not_collide() {
    let pair = pair().await;
    // Both directions open their first file slot (1). A STOP for the upload
    // must leave the independently numbered download usable.
    let mut upload = pair.client.open_uni(StreamKind::File).await.unwrap();
    let mut download = pair.server.open_uni(StreamKind::File).await.unwrap();
    upload.write_raw(b"upload").await.unwrap();
    download.write_raw(b"download").await.unwrap();
    let mut server_recv = accept(&pair.server, StreamKind::File).await;
    let mut client_recv = accept(&pair.client, StreamKind::File).await;
    server_recv.stop(3).unwrap();
    // A following control record acts as a barrier for STOP processing.
    pair.server_control.send(b"stopped upload").await.unwrap();
    bounded(pair.client_control.recv()).await.unwrap().unwrap();
    assert!(matches!(
        upload.write_raw(b"late").await,
        Err(ConnError::Closed)
    ));
    download.write_raw(b" still works").await.unwrap();
    download.finish().await.unwrap();
    assert_eq!(
        bounded(collect(&mut client_recv)).await.unwrap(),
        b"download still works"
    );

    let mut download = pair.server.open_uni(StreamKind::File).await.unwrap();
    download.write_raw(b"stop this download").await.unwrap();
    let mut recv = accept(&pair.client, StreamKind::File).await;
    recv.stop(3).unwrap();
    pair.client_control.send(b"stopped download").await.unwrap();
    bounded(pair.server_control.recv()).await.unwrap().unwrap();
    assert!(matches!(
        download.write_raw(b"late").await,
        Err(ConnError::Closed)
    ));
    assert_eq!(recv.read_raw(&mut [0; 10]).await.unwrap(), None);
}

#[tokio::test]
async fn invalid_flags_close_the_tls_connection_with_protocol_error() {
    let (mut raw, server) = raw_pair().await;
    raw.write_all(&rsp_transport::encode_tcp_record(FILE_CHANNEL, &file_chunk(1, 8, &[])).unwrap())
        .await
        .unwrap();
    let reason = bounded(server.closed()).await;
    assert_eq!(reason.code, Some(CloseCode::Protocol as u64));
    assert!(String::from_utf8_lossy(&reason.reason).contains("invalid file flags"));
}

#[tokio::test]
async fn too_many_open_files_close_the_connection_like_clipboard() {
    let (mut raw, server) = raw_pair().await;
    for slot in 1..=64 {
        raw.write_all(
            &rsp_transport::encode_tcp_record(FILE_CHANNEL, &file_chunk(slot, 0, &[])).unwrap(),
        )
        .await
        .unwrap();
        let _recv = accept(&server, StreamKind::File).await;
    }
    raw.write_all(
        &rsp_transport::encode_tcp_record(FILE_CHANNEL, &file_chunk(65, 0, &[])).unwrap(),
    )
    .await
    .unwrap();
    let reason = bounded(server.closed()).await;
    assert_eq!(reason.code, Some(CloseCode::Protocol as u64));
    assert!(String::from_utf8_lossy(&reason.reason).contains("too many virtual TCP streams"));
}

#[tokio::test]
async fn oversize_file_record_is_rejected_before_reading_payload() {
    let (mut raw, server) = raw_pair().await;
    let mut header = vec![FILE_CHANNEL];
    header.extend_from_slice(&((5 + FILE_TCP_CHUNK + 1) as u32).to_be_bytes());
    raw.write_all(&header).await.unwrap();
    let reason = bounded(server.closed()).await;
    assert_eq!(reason.code, Some(CloseCode::Protocol as u64));
    assert!(String::from_utf8_lossy(&reason.reason).contains("exceeds channel limit"));
}

#[test]
fn file_record_length_boundaries() {
    let payload = file_chunk(1, 0, &vec![0x5a; FILE_TCP_CHUNK]);
    let wire = rsp_transport::encode_tcp_record(FILE_CHANNEL, &payload).unwrap();
    let mut decoder = TcpRecordDecoder::default();
    assert_eq!(decoder.push(&wire).unwrap(), vec![(FILE_CHANNEL, payload)]);
    decoder.finish().unwrap();
    assert!(matches!(
        rsp_transport::encode_tcp_record(
            FILE_CHANNEL,
            &file_chunk(1, 0, &vec![0; FILE_TCP_CHUNK + 1])
        ),
        Err(ConnError::Protocol(_))
    ));
    let mut header = vec![FILE_CHANNEL];
    header.extend_from_slice(&((5 + FILE_TCP_CHUNK + 1) as u32).to_be_bytes());
    assert!(matches!(
        TcpRecordHeader::decode(&header),
        Err(ConnError::Protocol(_))
    ));
    assert!(matches!(
        TcpRecordDecoder::default().push(&header),
        Err(ConnError::Protocol(_))
    ));
}

#[test]
fn files_and_clipboard_share_the_live_stream_limit_and_release_slots() {
    let mut mux = TcpMuxState::default();
    for slot in 1..=32 {
        mux.receive(FILE_CHANNEL, file_chunk(slot, 0, &[])).unwrap();
        mux.receive(3, file_chunk(slot, 0, &[])).unwrap();
    }
    assert!(matches!(
        mux.receive(FILE_CHANNEL, file_chunk(33, 0, &[])),
        Err(ConnError::Protocol(_))
    ));
    mux.receive(FILE_CHANNEL, file_chunk(1, FILE_FLAG_FIN, &[]))
        .unwrap();
    mux.receive(FILE_CHANNEL, file_chunk(33, 0, &[])).unwrap();
    mux.receive(
        FILE_CHANNEL,
        file_chunk(2, FILE_FLAG_RESET, &2_u32.to_be_bytes()),
    )
    .unwrap();
    mux.receive(3, file_chunk(33, 0, &[])).unwrap();
    // Completed file slots never reopen, including out-of-order completions.
    assert_eq!(
        mux.receive(FILE_CHANNEL, file_chunk(2, 0, b"late"))
            .unwrap(),
        vec![MuxEvent::Ignore]
    );
    assert_eq!(
        mux.receive(FILE_CHANNEL, file_chunk(1, 0, b"late"))
            .unwrap(),
        vec![MuxEvent::Ignore]
    );
}

#[test]
fn malformed_file_payloads_are_protocol_errors_and_stop_opens_no_stream() {
    let mut mux = TcpMuxState::default();
    for payload in [
        Bytes::from_static(b"short"),
        file_chunk(0, 0, &[]),
        file_chunk(1, 8, &[]),
        file_chunk(1, FILE_FLAG_FIN, b"data"),
        file_chunk(1, FILE_FLAG_RESET, &[]),
        file_chunk(1, FILE_FLAG_STOP, &[0; 3]),
        file_chunk(1, FILE_FLAG_FIN | FILE_FLAG_RESET, &[0; 4]),
    ] {
        assert!(matches!(
            mux.receive(FILE_CHANNEL, payload),
            Err(ConnError::Protocol(_))
        ));
    }
    assert_eq!(
        mux.receive(
            FILE_CHANNEL,
            file_chunk(1, FILE_FLAG_STOP, &3_u32.to_be_bytes())
        )
        .unwrap(),
        vec![MuxEvent::Stop(1, 3)]
    );
    // STOP refers to the opposite direction, so receiving data on peer slot 1
    // still opens a stream.
    assert!(matches!(
        mux.receive(FILE_CHANNEL, file_chunk(1, 0, b"data"))
            .unwrap()[0],
        MuxEvent::Open(_)
    ));
}
