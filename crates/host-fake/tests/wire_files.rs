use host_core::{
    file_recv::partial_received,
    file_send::{self, SendError, SendOutcome},
    session::handlers::files,
};
use rsp_core::{
    features,
    file::{build_header, encode_stream_prefix, FileStreamReceiver, StreamEnd},
    pb::{self, FileHeader},
};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::time::timeout;

mod wire_support;
use wire_support::{client_hello, WireClient, WireHost};

const MIB: usize = 1024 * 1024;
const KIB: usize = 1024;

fn random_data(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(len);
    out
}

async fn header_for(
    host: &WireHost,
    transfer_id: &str,
    name: &str,
    full: &[u8],
    offset: u64,
) -> FileHeader {
    let path = host.state_dir.join(format!("hash-{transfer_id}.bin"));
    tokio::fs::write(&path, full).await.unwrap();
    let prepared = file_send::prepare(&path, Some(transfer_id.to_owned()))
        .await
        .unwrap();
    tokio::fs::remove_file(&path).await.unwrap();
    build_header(
        transfer_id,
        name,
        full.len() as u64,
        offset,
        prepared.sha256,
        prepared.mtime_us,
    )
    .unwrap()
}

async fn open_file_stream(client: &WireClient, header: &FileHeader) -> quinn::SendStream {
    let mut stream = client.open_typed(0x04).await;
    stream
        .write_all(&encode_stream_prefix(header).unwrap())
        .await
        .unwrap();
    stream
}

async fn finish_stream(mut stream: quinn::SendStream, data: &[u8]) {
    stream.write_all(data).await.unwrap();
    stream.finish().unwrap();
}

async fn stopped(stream: &mut quinn::SendStream, code: u32) {
    assert_eq!(
        timeout(Duration::from_secs(3), stream.stopped())
            .await
            .unwrap()
            .unwrap(),
        Some(code.into())
    );
    assert!(
        matches!(stream.write_all(&[1]).await, Err(quinn::WriteError::Stopped(c)) if c == code.into())
    );
}

async fn wait_until(within: Duration, mut condition: impl FnMut() -> bool) {
    timeout(within, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition timed out");
}

fn download_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

async fn wait_for_download(host: &WireHost, name: &str, size: usize) -> PathBuf {
    let path = host.downloads.join(name);
    if timeout(Duration::from_secs(2), async {
        while std::fs::metadata(&path).map_or(true, |metadata| metadata.len() != size as u64)
            || download_names(&host.downloads)
                .iter()
                .any(|entry| entry.starts_with(".rs-"))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_err()
    {
        panic!(
            "download {name:?} did not appear; directory contains {:?}",
            download_names(&host.downloads)
        );
    }
    path
}

fn parse_stream(bytes: &[u8]) -> (FileHeader, Vec<u8>) {
    let mut receiver = FileStreamReceiver::new();
    let fed = receiver.feed(bytes).expect("file stream parses");
    let header = fed.header.expect("file stream has a header");
    let mut data = Vec::new();
    if let Some(span) = fed.data {
        assert_eq!(span.offset, header.offset);
        data.extend_from_slice(span.bytes);
    }
    assert_eq!(
        receiver.finish().expect("file stream finishes"),
        StreamEnd::Complete {
            header: header.clone()
        }
    );
    (header, data)
}

#[tokio::test]
async fn controller_upload_is_saved_in_downloads() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;

    let id = "11111111111111111111111111111111";
    let body = b"hello world";
    let header = header_for(&host, id, "a.txt", body, 0).await;
    let stream = open_file_stream(&client, &header).await;
    finish_stream(stream, body).await;

    let path = wait_for_download(&host, "a.txt", body.len()).await;
    assert_eq!(tokio::fs::read(path).await.unwrap(), body);
    assert_eq!(download_names(&host.downloads), ["a.txt"]);
}

#[tokio::test]
async fn same_name_does_not_overwrite() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;

    let first = b"hello world";
    let first_header =
        header_for(&host, "22222222222222222222222222222222", "a.txt", first, 0).await;
    finish_stream(open_file_stream(&client, &first_header).await, first).await;
    let original = wait_for_download(&host, "a.txt", first.len()).await;

    let second = b"HELLO WORLD";
    let second_header = header_for(
        &host,
        "33333333333333333333333333333333",
        "a.txt",
        second,
        0,
    )
    .await;
    finish_stream(open_file_stream(&client, &second_header).await, second).await;
    let numbered = wait_for_download(&host, "a (1).txt", second.len()).await;

    assert_eq!(tokio::fs::read(original).await.unwrap(), first);
    assert_eq!(tokio::fs::read(numbered).await.unwrap(), second);
    assert_eq!(download_names(&host.downloads), ["a (1).txt", "a.txt"]);
}

#[tokio::test]
async fn interrupted_upload_resumes_from_partial_file() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;

    let id = "44444444444444444444444444444444";
    let body = b"hello world";
    let header = header_for(&host, id, "resume.txt", body, 0).await;
    let mut first = open_file_stream(&client, &header).await;
    first.write_all(&body[..5]).await.unwrap();
    wait_until(Duration::from_secs(2), || {
        partial_received(&host.downloads, id).unwrap() == Some(5)
    })
    .await;
    first.reset(0u32.into()).unwrap();
    wait_until(Duration::from_secs(2), || {
        partial_received(&host.downloads, id).unwrap() == Some(5)
    })
    .await;

    let resumed = build_header(
        id,
        "resume.txt",
        header.size,
        5,
        header.sha256.as_slice().try_into().unwrap(),
        header.mtime_us,
    )
    .unwrap();
    finish_stream(open_file_stream(&client, &resumed).await, &body[5..]).await;

    let path = wait_for_download(&host, "resume.txt", body.len()).await;
    assert_eq!(tokio::fs::read(path).await.unwrap(), body);
    assert_eq!(download_names(&host.downloads), ["resume.txt"]);
    assert!(!host.downloads.join(format!(".rs-{id}.part")).exists());
    assert!(!host.downloads.join(format!(".rs-{id}.json")).exists());
}

#[tokio::test]
async fn checksum_failure_removes_the_partial_file() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    std::fs::write(host.downloads.join("keep.txt"), b"keep").unwrap();

    let id = "55555555555555555555555555555555";
    let body = b"corrupt me";
    let header = build_header(
        id,
        "bad.txt",
        body.len() as u64,
        0,
        [0x5a; 32],
        1_735_689_600_000_000,
    )
    .unwrap();
    finish_stream(open_file_stream(&client, &header).await, body).await;

    wait_until(Duration::from_secs(2), || {
        download_names(&host.downloads) == ["keep.txt"]
    })
    .await;
    assert!(!host.downloads.join("bad.txt").exists());
}

#[tokio::test]
async fn viewer_upload_is_stopped_and_ignored() {
    let mut options = wire_support::WireOptions::default();
    options.options.view_only = true;
    let host = WireHost::start_with(options).await;
    let mut viewer = host.connect().await;
    let (_, auth) = viewer.hello().await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);

    let mut stream = viewer.open_typed(0x04).await;
    let _ = stream.write_all(&[1, 2, 3]).await;
    stopped(&mut stream, 6).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(download_names(&host.downloads).is_empty());
}

#[tokio::test]
async fn fifth_concurrent_upload_is_stopped() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;

    let body = b"one";
    let mut headers = Vec::new();
    for index in 0..5 {
        headers.push(
            header_for(
                &host,
                &format!("{index:032x}"),
                &format!("{index}.txt"),
                body,
                0,
            )
            .await,
        );
    }

    let mut streams = Vec::new();
    for header in &headers {
        streams.push(open_file_stream(&client, header).await);
    }
    stopped(&mut streams[4], 4).await;

    for stream in streams.drain(..4) {
        finish_stream(stream, body).await;
    }
    for header in headers.iter().take(4) {
        let _ = wait_for_download(&host, &header.name, body.len()).await;
    }
}

#[tokio::test]
async fn host_sends_a_file_on_a_typed_stream() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let key = host.conns()[0].key;
    let body = random_data(MIB, 0x1234_5678_9abc_def0);
    let path = host.state_dir.join("host-send.bin");
    tokio::fs::write(&path, &body).await.unwrap();
    let prepared = file_send::prepare(&path, None).await.unwrap();

    let (outcome, (kind, mut recv)) = tokio::join!(
        files::send_file(&host.mgr, key, &path),
        client.accept_typed(),
    );
    assert_eq!(
        outcome.expect("send succeeds"),
        SendOutcome::Completed { sent: MIB as u64 }
    );
    assert_eq!(kind, 0x04, "the stream must carry the file channel type");

    let bytes = recv
        .read_to_end(MIB + 16 * KIB)
        .await
        .expect("file stream reads to FIN");
    let (header, received) = parse_stream(&bytes);
    assert_eq!(header.name, "host-send.bin");
    assert_eq!(header.sha256, prepared.sha256);
    assert_eq!(received, body);
}

#[tokio::test]
async fn disconnect_keeps_the_partial_upload() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;

    let id = "66666666666666666666666666666666";
    let body = random_data(MIB, 0x0fed_cba9_8765_4321);
    let header = header_for(&host, id, "large.bin", &body, 0).await;
    let mut stream = open_file_stream(&client, &header).await;
    stream.write_all(&body[..100 * KIB]).await.unwrap();
    wait_until(Duration::from_secs(2), || {
        partial_received(&host.downloads, id)
            .expect("partial state is readable")
            .is_some_and(|received| received > 0)
    })
    .await;
    client.conn.close(0u32.into(), b"test disconnect");

    wait_until(Duration::from_secs(1), || {
        partial_received(&host.downloads, id)
            .expect("partial state survives disconnect")
            .is_some_and(|received| received > 0)
    })
    .await;
    assert!(!host.downloads.join("large.bin").exists());
}

#[tokio::test]
async fn server_hello_advertises_file_transfer() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    let (server, _) = client.hello().await;
    assert_ne!(server.features & features::FILE_TRANSFER, 0);
}

#[tokio::test]
async fn send_file_requires_a_known_connection() {
    let host = WireHost::start().await;
    let path = host.state_dir.join("unused.bin");
    tokio::fs::write(&path, b"x").await.unwrap();
    let error = files::send_file(&host.mgr, host.mgr.next_key(), &path)
        .await
        .expect_err("unknown connection cannot send");
    assert!(
        matches!(error, SendError::Io(error) if error.kind() == std::io::ErrorKind::NotConnected)
    );
}

#[tokio::test]
async fn send_file_requires_the_client_feature() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    let mut hello = client_hello();
    hello.features &= !features::FILE_TRANSFER;
    client.hello_with(hello).await;
    let key = host.conns()[0].key;

    let path = host.state_dir.join("unsupported.bin");
    tokio::fs::write(&path, b"x").await.unwrap();
    let error = files::send_file(&host.mgr, key, &path)
        .await
        .expect_err("client without FILE_TRANSFER cannot receive files");
    assert!(
        matches!(error, SendError::Io(error) if error.kind() == std::io::ErrorKind::NotConnected)
    );
}
