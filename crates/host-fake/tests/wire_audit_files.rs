use host_core::audit::{AuditPath, HostAuditRecord};
use host_core::{
    file_recv::partial_received,
    file_send::{self, SendOutcome},
    session::handlers::files,
};
use rsp_core::{
    file::{build_header, encode_stream_prefix, FileStreamReceiver, StreamEnd},
    pb::FileHeader,
};
use serde_json::json;
use std::time::Duration;

mod wire_support;
use wire_support::{device_id, Dev, WireClient, WireHost, WireOptions};

async fn audited_host() -> WireHost {
    WireHost::start_with(WireOptions {
        audit: true,
        ..Default::default()
    })
    .await
}

async fn header_for(host: &WireHost, transfer_id: &str, name: &str, body: &[u8]) -> FileHeader {
    let path = host.state_dir.join(format!("hash-{transfer_id}.bin"));
    tokio::fs::write(&path, body).await.unwrap();
    let prepared = file_send::prepare(&path, Some(transfer_id.to_owned()))
        .await
        .unwrap();
    tokio::fs::remove_file(&path).await.unwrap();
    build_header(
        transfer_id,
        name,
        body.len() as u64,
        0,
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

async fn wait_for_file(host: &WireHost, name: &str, size: usize) {
    let path = host.downloads.join(name);
    host.wait_until(|| {
        std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() == size as u64)
    })
    .await;
}

async fn wait_for_receive_started(host: &WireHost, transfer_id: &str) {
    host.wait_until(|| {
        partial_received(&host.downloads, transfer_id)
            .expect("partial state is readable")
            .is_some()
    })
    .await;
}

fn file_events(host: &WireHost) -> Vec<HostAuditRecord> {
    host.audit_events()
        .into_iter()
        .filter(|record| record.event == "file")
        .collect()
}

fn assert_no_file_or_path_key(value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                assert!(
                    !key.contains("name") && !key.contains("path"),
                    "file audit detail must not contain {key:?}: {value:?}"
                );
                assert_no_file_or_path_key(value);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                assert_no_file_or_path_key(value);
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn completed_upload_records_one_file_audit() {
    let host = audited_host().await;
    let mut client = host.connect().await;
    client.hello().await;

    let id = "11111111111111111111111111111111";
    let body = b"abc";
    let header = header_for(&host, id, "upload.txt", body).await;
    let stream = open_file_stream(&client, &header).await;
    wait_for_receive_started(&host, id).await;
    host.clock.skip(Duration::from_secs(2));
    finish_stream(stream, body).await;
    wait_for_file(&host, "upload.txt", body.len()).await;
    host.wait_until(|| file_events(&host).len() == 1).await;

    let events = file_events(&host);
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(
        event.detail,
        Some(json!({"op": "upload", "files": 1, "bytes": 3}))
    );
    assert_eq!(event.duration_s, Some(2));
    assert_eq!(event.path, Some(AuditPath::Lan));
    assert_eq!(event.device_id.as_deref(), Some(device_id(Dev::A).as_str()));
    assert_no_file_or_path_key(event.detail.as_ref().unwrap());
}

#[tokio::test]
async fn completed_download_records_one_file_audit() {
    let host = audited_host().await;
    let mut client = host.connect().await;
    client.hello().await;
    let key = host.conns()[0].key;
    let body = b"abcde";
    let path = host.state_dir.join("download.bin");
    tokio::fs::write(&path, body).await.unwrap();

    let (outcome, (kind, mut recv)) = tokio::join!(
        files::send_file(&host.mgr, key, &path),
        client.accept_typed(),
    );
    assert_eq!(outcome.unwrap(), SendOutcome::Completed { sent: 5 });
    assert_eq!(kind, 0x04);
    let bytes = recv.read_to_end(16 * 1024).await.unwrap();
    let mut receiver = FileStreamReceiver::new();
    let fed = receiver.feed(&bytes).unwrap();
    let header = fed.header.unwrap();
    assert_eq!(header.name, "download.bin");
    assert_eq!(fed.data.unwrap().bytes, body);
    assert_eq!(receiver.finish().unwrap(), StreamEnd::Complete { header });
    host.wait_until(|| file_events(&host).len() == 1).await;

    let events = file_events(&host);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].detail,
        Some(json!({"op": "download", "files": 1, "bytes": 5}))
    );
    assert_eq!(
        events[0].device_id.as_deref(),
        Some(device_id(Dev::A).as_str())
    );
    assert_no_file_or_path_key(events[0].detail.as_ref().unwrap());
}

#[tokio::test]
async fn interrupted_upload_records_nothing() {
    let host = audited_host().await;
    let mut client = host.connect().await;
    client.hello().await;

    let id = "22222222222222222222222222222222";
    let body = b"hello";
    let header = header_for(&host, id, "partial.txt", body).await;
    let mut stream = open_file_stream(&client, &header).await;
    stream.write_all(&body[..2]).await.unwrap();
    wait_for_receive_started(&host, id).await;
    client.conn.close(0u32.into(), b"test disconnect");
    host.wait_until(|| host.conns().is_empty()).await;
    host.clock.skip(Duration::from_secs(3));

    assert!(file_events(&host).is_empty());
}

#[tokio::test]
async fn viewer_upload_is_rejected_without_an_audit() {
    let mut options = WireOptions {
        audit: true,
        ..Default::default()
    };
    options.options.view_only = true;
    let host = WireHost::start_with(options).await;
    let mut viewer = host.connect().await;
    let (_, auth) = viewer.hello().await;
    assert_eq!(auth.role, rsp_core::pb::Role::Viewer as i32);

    let mut stream = viewer.open_typed(0x04).await;
    let _ = stream.write_all(&[1, 2, 3]).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), stream.stopped())
            .await
            .unwrap()
            .unwrap(),
        Some(rsp_core::file_ctl::STREAM_NOT_ALLOWED.into())
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(file_events(&host).is_empty());
}

#[tokio::test]
async fn completed_upload_without_auditing_does_not_panic() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;

    let id = "33333333333333333333333333333333";
    let body = b"abc";
    let header = header_for(&host, id, "unaudited.txt", body).await;
    finish_stream(open_file_stream(&client, &header).await, body).await;
    wait_for_file(&host, "unaudited.txt", body.len()).await;
    assert!(host.audit_events().is_empty());
}
