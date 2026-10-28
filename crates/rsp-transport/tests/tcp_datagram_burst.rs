use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use prost::Message;
use rsp_core::{audio, pb};
use rsp_transport::{
    connect_tcp, encode_tcp_record,
    identity::{CertKind, LocalIdentity},
    tls::{client_config, server_config, ServerPin},
    CloseCode, Conn, ControlStream, DatagramKind, TcpRecordHeader,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_rustls::{server::TlsStream, TlsAcceptor};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

async fn pair() -> (Conn, ControlStream, TlsStream<TcpStream>) {
    let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let app = LocalIdentity::generate_software(CertKind::App).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config(&host).unwrap()));
    let config = Arc::new(client_config(&app, ServerPin::Fingerprint(host.fingerprint())).unwrap());
    let (client, server) =
        tokio::join!(connect_tcp(listener.local_addr().unwrap(), config), async {
            let (socket, _) = listener.accept().await.unwrap();
            acceptor.accept(socket).await.unwrap()
        });
    let client = client.unwrap();
    let control = client.open_control().await.unwrap();
    (client, control, server)
}

fn audio_record(seq: u16) -> Bytes {
    audio::encode(
        audio::AudioHeader {
            seq,
            capture_ts_us: 1_000_000 + u64::from(seq) * 10_000,
        },
        &[0xf8, seq as u8, 0xff],
    )
    .unwrap()
    .slice(1..)
}

fn append_audio(batch: &mut BytesMut, seq: u16) {
    batch.extend_from_slice(
        &encode_tcp_record(DatagramKind::Audio as u8, &audio_record(seq)).unwrap(),
    );
}

fn append_ping(batch: &mut BytesMut, id: u64) {
    let message: pb::Envelope = pb::Ping { t1_us: id }.into();
    batch.extend_from_slice(&encode_tcp_record(0, &message.encode_to_vec()).unwrap());
}

async fn receive_ping(control: &ControlStream, id: u64) {
    let message = pb::Envelope::decode(control.recv().await.unwrap().unwrap()).unwrap();
    assert!(matches!(message.msg, Some(pb::envelope::Msg::Ping(ping)) if ping.t1_us == id));
}

async fn pong(control: &ControlStream, server: &mut TlsStream<TcpStream>, id: u64) {
    let message: pb::Envelope = pb::Pong {
        t1_us: id,
        t2_us: id + 1,
    }
    .into();
    control.send(&message.encode_to_vec()).await.unwrap();
    let mut header = [0; 5];
    server.read_exact(&mut header).await.unwrap();
    let header = TcpRecordHeader::decode(&header).unwrap();
    assert_eq!(header.channel, 0);
    let mut payload = vec![0; header.length as usize];
    server.read_exact(&mut payload).await.unwrap();
    assert_eq!(pb::Envelope::decode(payload.as_slice()).unwrap(), message);
}

#[tokio::test]
async fn tcp_receives_sixty_four_audio_records_without_drops() {
    timeout(TEST_TIMEOUT, async {
        let (client, control, mut server) = pair().await;
        let mut batch = BytesMut::new();
        for seq in 0..64 {
            append_audio(&mut batch, seq);
        }
        append_ping(&mut batch, 1);
        server.write_all(&batch).await.unwrap();
        server.flush().await.unwrap();

        // The trailing control record proves the entire burst was dispatched
        // before the application consumes any queued media.
        receive_ping(&control, 1).await;
        assert_eq!(client.dropped_datagram_count(), 0);
        for seq in 0..64 {
            assert_eq!(
                client.recv_datagram().await.unwrap(),
                (DatagramKind::Audio, audio_record(seq))
            );
        }
        assert_eq!(client.dropped_datagram_count(), 0);
        client.close(CloseCode::Normal, b"test done");
    })
    .await
    .expect("TCP audio burst deadline");
}

#[derive(Default)]
struct DropLogs(AtomicUsize);

impl tracing::Subscriber for DropLogs {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == "rsp_transport::tcp"
            && event.metadata().fields().field("dropped").is_some()
        {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn tcp_overflow_counts_drops_without_blocking_control_or_closing() {
    let logs = Arc::new(DropLogs::default());
    let _subscriber = tracing::subscriber::set_default(logs.clone());
    timeout(TEST_TIMEOUT, async {
        const RECORDS: u16 = 4096;
        let (client, control, mut server) = pair().await;
        let mut batch = BytesMut::new();
        for seq in 0..RECORDS {
            append_audio(&mut batch, seq);
            if seq == RECORDS / 2 {
                append_ping(&mut batch, 1);
            }
        }
        append_ping(&mut batch, 2);
        server.write_all(&batch).await.unwrap();
        server.flush().await.unwrap();

        // Leave media unread while control records cross the overflowing queue
        // in both directions, midway through the burst and after it.
        for id in [1, 2] {
            receive_ping(&control, id).await;
            assert!(client.dropped_datagram_count() > 0);
            pong(&control, &mut server, id).await;
        }
        let dropped = client.dropped_datagram_count();
        assert!(dropped > 0 && dropped < u64::from(RECORDS));
        assert_eq!(client.clone().dropped_datagram_count(), dropped);
        assert_eq!(client.unknown_datagram_count(), 0);
        let log_count = logs.0.load(Ordering::Relaxed);
        // One initial log, then at most one per second within the total deadline.
        assert!(
            log_count > 0 && log_count as u64 <= TEST_TIMEOUT.as_secs() + 1,
            "drop logs: {log_count}"
        );

        let retained = u64::from(RECORDS) - dropped;
        let mut previous = None;
        for _ in 0..retained {
            let (kind, payload) = client.recv_datagram().await.unwrap();
            assert_eq!(kind, DatagramKind::Audio);
            let seq = u16::from_be_bytes([payload[0], payload[1]]);
            assert!(previous.is_none_or(|previous| seq > previous));
            assert_eq!(payload, audio_record(seq));
            previous = Some(seq);
        }

        let mut probe = BytesMut::new();
        append_audio(&mut probe, RECORDS);
        append_ping(&mut probe, 3);
        server.write_all(&probe).await.unwrap();
        server.flush().await.unwrap();
        receive_ping(&control, 3).await;
        pong(&control, &mut server, 3).await;
        assert_eq!(
            client.recv_datagram().await.unwrap(),
            (DatagramKind::Audio, audio_record(RECORDS))
        );
        assert_eq!(client.dropped_datagram_count(), dropped);
        client.close(CloseCode::Normal, b"test done");
    })
    .await
    .expect("TCP overflow/control deadline");
}
