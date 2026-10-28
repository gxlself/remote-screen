use host_core::file_send;
use rsp_core::{
    file::build_header,
    file::encode_stream_prefix,
    pb::{self, file_status::State, FileHeader, FileStatus},
};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

mod wire_support;
use wire_support::{Dev, OffsetClock, WireClient, WireHost, WireOptions};

const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MIB: usize = 1024 * 1024;

async fn header(host: &WireHost, id: &str, body: &[u8]) -> FileHeader {
    let path = host.state_dir.join("source.bin");
    tokio::fs::write(&path, body).await.unwrap();
    let prepared = file_send::prepare(&path, Some(id.to_owned()))
        .await
        .unwrap();
    tokio::fs::remove_file(path).await.unwrap();
    build_header(
        id,
        "restart.bin",
        body.len() as u64,
        0,
        prepared.sha256,
        prepared.mtime_us,
    )
    .unwrap()
}

async fn upload(client: &WireClient, header: &FileHeader, body: &[u8]) {
    let mut stream = client.open_typed(0x04).await;
    stream
        .write_all(&encode_stream_prefix(header).unwrap())
        .await
        .unwrap();
    stream.write_all(body).await.unwrap();
    stream.finish().unwrap();
}

async fn status(client: &mut WireClient) -> FileStatus {
    client
        .next_skipping_session_state("FileStatus", |message| match message {
            pb::envelope::Msg::FileStatus(status) => Some(status.clone()),
            _ => None,
        })
        .await
}

async fn query(client: &mut WireClient, id: &str) -> FileStatus {
    client
        .send(pb::FileQuery {
            request_id: 7,
            transfer_id: id.to_owned(),
        })
        .await;
    status(client).await
}

fn names(path: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn state_dir(tag: &str) -> std::path::PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("wire-files-restart-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

async fn restart(state_dir: &Path, clock: Arc<OffsetClock>) -> WireHost {
    WireHost::start_with(WireOptions {
        state_dir: Some(state_dir.to_owned()),
        clock: Some(clock),
        ..Default::default()
    })
    .await
}

#[tokio::test]
async fn completed_upload_survives_host_restart() {
    let clock = Arc::new(OffsetClock::manual());
    let initial_state_dir = state_dir("completed");
    let host = WireHost::start_with(WireOptions {
        clock: Some(clock.clone()),
        state_dir: Some(initial_state_dir),
        ..Default::default()
    })
    .await;
    let state_dir = host.state_dir.clone();
    let mut client = host.connect_as(Dev::A, 0).await;
    client.hello().await;
    let body = vec![0x35; MIB];
    let header = header(&host, ID, &body).await;
    upload(&client, &header, &body).await;
    let completed = status(&mut client).await;
    assert_eq!(completed.state, State::Completed as i32);
    assert_eq!(completed.received, body.len() as u64);
    host.wait_until(|| state_dir.join("completed-uploads.json").is_file())
        .await;
    drop(client);
    drop(host);

    let host = restart(&state_dir, clock).await;
    let mut client = host.connect_as(Dev::A, 0).await;
    client.hello().await;
    let queried = query(&mut client, ID).await;
    assert_eq!(queried.state, State::Completed as i32);
    assert_eq!(queried.saved_name, "restart.bin");
    assert_eq!(queried.received, body.len() as u64);
    assert_eq!(names(&host.downloads), vec!["restart.bin"]);
    drop(client);
    drop(host);
    let _ = std::fs::remove_dir_all(state_dir);
}

#[tokio::test]
async fn expired_entry_is_dropped_on_load() {
    let clock = Arc::new(OffsetClock::manual());
    let initial_state_dir = state_dir("expired");
    let host = WireHost::start_with(WireOptions {
        clock: Some(clock.clone()),
        state_dir: Some(initial_state_dir),
        ..Default::default()
    })
    .await;
    let state_dir = host.state_dir.clone();
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x36; MIB];
    let header = header(&host, ID, &body).await;
    upload(&client, &header, &body).await;
    assert_eq!(status(&mut client).await.state, State::Completed as i32);
    host.wait_until(|| state_dir.join("completed-uploads.json").is_file())
        .await;
    drop(client);
    drop(host);
    clock.skip(Duration::from_secs(24 * 3600 + 1));

    let host = restart(&state_dir, clock).await;
    let mut client = host.connect().await;
    client.hello().await;
    assert_eq!(query(&mut client, ID).await.state, State::Unknown as i32);
    drop(client);
    drop(host);
    let _ = std::fs::remove_dir_all(state_dir);
}

#[tokio::test]
async fn corrupt_registry_is_quarantined() {
    let clock = Arc::new(OffsetClock::manual());
    let initial_state_dir = state_dir("corrupt");
    let host = WireHost::start_with(WireOptions {
        clock: Some(clock.clone()),
        state_dir: Some(initial_state_dir),
        ..Default::default()
    })
    .await;
    let state_dir = host.state_dir.clone();
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x37; MIB];
    let header = header(&host, ID, &body).await;
    upload(&client, &header, &body).await;
    assert_eq!(status(&mut client).await.state, State::Completed as i32);
    host.wait_until(|| state_dir.join("completed-uploads.json").is_file())
        .await;
    drop(client);
    drop(host);
    tokio::fs::write(state_dir.join("completed-uploads.json"), b"garbage")
        .await
        .unwrap();

    let host = restart(&state_dir, clock).await;
    let mut client = host.connect().await;
    client.hello().await;
    assert_eq!(query(&mut client, ID).await.state, State::Unknown as i32);
    assert!(state_dir.join("completed-uploads.json.bad").is_file());
    drop(client);
    drop(host);
    let _ = std::fs::remove_dir_all(state_dir);
}

#[tokio::test]
async fn no_state_dir_keeps_memory_only() {
    let services = host_core::session::manager::HostServices {
        state_dir: None,
        ..Default::default()
    };
    let _host = host_core::session::handlers::files::Host::new(&services);
    assert!(!services
        .downloads_dir
        .join("completed-uploads.json")
        .exists());
}
