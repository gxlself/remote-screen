use host_core::{file_recv::partial_received, file_send};
use rsp_core::{
    file::{build_header, encode_stream_prefix, FileStreamReceiver, StreamEnd},
    file_ctl,
    pb::{self, file_status::State, FileError, FileHeader, FileStatus},
};
use rsp_transport::{StreamKind, UniSend};
use std::{path::PathBuf, time::Duration};
use tokio::time::timeout;

mod wire_support;
use wire_support::{Dev, WireHost, WireOptions, WireTcpClient};

const MIB: usize = 1 << 20;
const ID: &str = "11111111111111111111111111111111";

struct BrowseRoot(PathBuf);

impl BrowseRoot {
    fn new() -> Self {
        let root = Self(std::env::temp_dir().join(format!(
            "rs-tcp-files-{}-{}",
            std::process::id(),
            rsp_core::file::new_transfer_id().unwrap()
        )));
        std::fs::create_dir_all(&root.0).unwrap();
        root
    }
}

impl Drop for BrowseRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn body() -> Vec<u8> {
    (0..2 * MIB).map(|i| (i % 251) as u8).collect()
}

async fn header(host: &WireHost, body: &[u8]) -> FileHeader {
    let path = host.state_dir.join("hash-source.bin");
    tokio::fs::write(&path, body).await.unwrap();
    let prepared = file_send::prepare(&path, Some(ID.to_owned()))
        .await
        .unwrap();
    tokio::fs::remove_file(path).await.unwrap();
    build_header(
        ID,
        "tcp-upload.bin",
        body.len() as u64,
        0,
        prepared.sha256,
        prepared.mtime_us,
    )
    .unwrap()
}

async fn open(client: &WireTcpClient, header: &FileHeader) -> UniSend {
    let mut stream = client.conn.open_uni(StreamKind::File).await.unwrap();
    stream
        .write_raw(&encode_stream_prefix(header).unwrap())
        .await
        .unwrap();
    stream
}

async fn status(client: &WireTcpClient) -> FileStatus {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::FileStatus(status) => Some(status.clone()),
            _ => None,
        })
        .await
}

#[tokio::test]
async fn tcp_upload_two_mib_is_saved_and_reports_completed() {
    let host = WireHost::start().await;
    let client = host.connect_tcp(Dev::A, 0).await;
    client.hello().await;
    let body = body();
    let header = header(&host, &body).await;
    let mut stream = open(&client, &header).await;
    timeout(Duration::from_secs(5), async {
        stream.write_raw(&body).await.unwrap();
        stream.finish().await.unwrap();
    })
    .await
    .unwrap();
    let status = status(&client).await;
    assert_eq!(status.transfer_id, ID);
    assert_eq!(status.state, State::Completed as i32);
    assert_eq!(status.error, FileError::None as i32);
    assert_eq!(status.received, body.len() as u64);
    assert_eq!(status.saved_name, header.name);
    assert_eq!(
        tokio::fs::read(host.downloads.join(status.saved_name))
            .await
            .unwrap(),
        body
    );
    client.ping().await;
}

#[tokio::test]
async fn tcp_file_request_returns_a_file_stream_with_matching_content() {
    let root = BrowseRoot::new();
    let mut options = WireOptions::default();
    options.config.files.browse_roots = vec![root.0.to_string_lossy().into_owned()];
    let host = WireHost::start_with(options).await;
    let source = root.0.join("tcp-download.bin");
    let body = body();
    tokio::fs::write(&source, &body).await.unwrap();
    let client = host.connect_tcp(Dev::A, 0).await;
    client.hello().await;
    client
        .send(pb::FileRequest {
            request_id: 17,
            transfer_id: ID.to_owned(),
            path: source.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .await;
    let received = timeout(Duration::from_secs(5), async {
        let (kind, mut stream) = tokio::select! {
            stream = client.conn.accept_uni() => stream.unwrap(),
            failure = status(&client) => panic!("FileRequest failed: {failure:?}"),
        };
        assert_eq!(kind, StreamKind::File);
        let mut receiver = FileStreamReceiver::new();
        let mut buf = [0; 32 * 1024];
        let mut received = Vec::new();
        while let Some(n) = stream.read_raw(&mut buf).await.unwrap() {
            let fed = receiver.feed(&buf[..n]).unwrap();
            if let Some(header) = fed.header {
                assert_eq!(header.transfer_id, ID);
                assert_eq!(header.name, "tcp-download.bin");
                assert!(header.dest_dir.is_empty());
                assert_eq!(header.size, body.len() as u64);
            }
            if let Some(data) = fed.data {
                received.extend_from_slice(data.bytes);
            }
        }
        assert!(matches!(
            receiver.finish().unwrap(),
            StreamEnd::Complete { .. }
        ));
        received
    })
    .await
    .unwrap();
    assert_eq!(received, body);
    client.ping().await;
}

#[tokio::test]
async fn tcp_reset_halfway_preserves_partial_for_file_query() {
    let host = WireHost::start().await;
    let client = host.connect_tcp(Dev::A, 0).await;
    client.hello().await;
    let body = body();
    let header = header(&host, &body).await;
    let mut stream = open(&client, &header).await;
    stream.write_raw(&body[..MIB]).await.unwrap();
    host.wait_until(|| partial_received(&host.downloads, ID).unwrap() == Some(MIB as u64))
        .await;
    stream.reset(file_ctl::STREAM_PAUSED).unwrap();
    let status = timeout(Duration::from_secs(3), async {
        loop {
            client
                .send(pb::FileQuery {
                    request_id: 23,
                    transfer_id: ID.to_owned(),
                })
                .await;
            let result = status(&client).await;
            assert_eq!(result.request_id, 23);
            assert_eq!(result.transfer_id, ID);
            if result.state != State::Receiving as i32 {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status.state, State::Partial as i32);
    assert_eq!(status.error, FileError::None as i32);
    assert_eq!(status.received, MIB as u64);
    assert_eq!(
        tokio::fs::read(host.downloads.join(format!(".rs-{ID}.part")))
            .await
            .unwrap(),
        body[..MIB]
    );
    assert!(!host.downloads.join(header.name).exists());
    client.ping().await;
}

#[tokio::test]
async fn tcp_disabled_stream_with_header_reports_failed_status() {
    let mut options = WireOptions::default();
    options.config.files.from_phone = false;
    let host = WireHost::start_with(options).await;
    let client = host.connect_tcp(Dev::A, 0).await;
    client.hello().await;
    let body = body();
    let header = header(&host, &body).await;
    let _stream = open(&client, &header).await;
    let result = status(&client).await;
    assert_eq!(result.transfer_id, ID);
    assert_eq!(result.state, State::Failed as i32);
    assert_eq!(result.error, FileError::Disabled as i32);
    assert_eq!(result.received, 0);
    client.ping().await;
}
