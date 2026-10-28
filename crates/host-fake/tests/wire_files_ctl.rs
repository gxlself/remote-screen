use host_core::{file_recv::partial_received, file_send};
use rsp_core::{
    features,
    file::{build_header, encode_stream_prefix},
    file_ctl,
    pb::{self, file_status::State, FileError, FileHeader, FileStatus},
};
use std::{
    path::Path,
    time::{Duration, SystemTime},
};
use tokio::time::timeout;

mod wire_support;
use wire_support::{client_hello, Dev, WireClient, WireHost, WireOptions};

const MIB: usize = 1024 * 1024;
const PREFIX: usize = 300 * 1024;
const ID: &str = "11111111111111111111111111111111";
const ID2: &str = "22222222222222222222222222222222";

async fn header(host: &WireHost, id: &str, body: &[u8]) -> FileHeader {
    let path = host.state_dir.join(format!("hash-{id}"));
    tokio::fs::write(&path, body).await.unwrap();
    let prepared = file_send::prepare(&path, Some(id.to_owned()))
        .await
        .unwrap();
    tokio::fs::remove_file(path).await.unwrap();
    build_header(
        id,
        "a.txt",
        body.len() as u64,
        0,
        prepared.sha256,
        prepared.mtime_us,
    )
    .unwrap()
}

async fn open(client: &WireClient, header: &FileHeader) -> quinn::SendStream {
    let mut stream = client.open_typed(0x04).await;
    stream
        .write_all(&encode_stream_prefix(header).unwrap())
        .await
        .unwrap();
    stream
}

async fn upload(client: &WireClient, header: &FileHeader, body: &[u8]) {
    let mut stream = open(client, header).await;
    stream.write_all(body).await.unwrap();
    stream.finish().unwrap();
}

async fn status(client: &mut WireClient) -> FileStatus {
    client
        .next_skipping_session_state("FileStatus", |msg| match msg {
            pb::envelope::Msg::FileStatus(status) => Some(status.clone()),
            _ => None,
        })
        .await
}

fn assert_state(status: &FileStatus, id: &str, state: State, error: FileError) {
    assert_eq!(status.transfer_id, id);
    assert_eq!(status.state, state as i32, "{status:?}");
    assert_eq!(status.error, error as i32, "{status:?}");
}

async fn query(client: &mut WireClient, id: &str, request_id: u32) -> FileStatus {
    client
        .send(pb::FileQuery {
            request_id,
            transfer_id: id.to_owned(),
        })
        .await;
    let result = status(client).await;
    assert_eq!(result.request_id, request_id);
    assert_eq!(result.transfer_id, id);
    result
}

async fn wait_partial(client: &mut WireClient, id: &str) -> FileStatus {
    timeout(Duration::from_secs(2), async {
        loop {
            let result = query(client, id, 17).await;
            if result.state != State::Receiving as i32 {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("interrupted receive releases its registration")
}

async fn partial_upload(
    host: &WireHost,
    client: &WireClient,
    header: &FileHeader,
    body: &[u8],
) -> quinn::SendStream {
    let mut stream = open(client, header).await;
    stream.write_all(&body[..PREFIX]).await.unwrap();
    host.wait_until(|| {
        partial_received(&host.downloads, &header.transfer_id).unwrap() == Some(PREFIX as u64)
    })
    .await;
    stream
}

async fn cancel(client: &mut WireClient, id: &str) -> FileStatus {
    client
        .send(pb::FileCancel {
            transfer_id: id.to_owned(),
        })
        .await;
    status(client).await
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

async fn stopped(stream: &mut quinn::SendStream, code: u32) {
    assert_eq!(
        timeout(Duration::from_secs(3), stream.stopped())
            .await
            .unwrap()
            .unwrap(),
        Some(code.into())
    );
}

async fn second_connection(host: &WireHost, device: Dev) -> WireClient {
    // FILES and CONTROL can coexist for one device after, without
    // replacing A or taking its controller seat.
    let mut client = host.connect_as(device, 0).await;
    client
        .send(pb::ClientHello {
            intent: pb::Intent::Files as i32,
            features: client_hello().features | features::FILE_BROWSE,
            required_features: features::FILE_BROWSE,
            ..client_hello()
        })
        .await;
    let auth = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::AuthResult(auth) => Some(auth.clone()),
            _ => None,
        })
        .await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    client
}

#[tokio::test]
async fn completed_upload_reports_full_size_and_actual_numbered_name() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x35; MIB];
    for (id, saved_name) in [(ID, "a.txt"), (ID2, "a (1).txt")] {
        let header = header(&host, id, &body).await;
        upload(&client, &header, &body).await;
        let result = status(&mut client).await;
        assert_state(&result, id, State::Completed, FileError::None);
        assert_eq!(result.request_id, 0);
        assert_eq!(result.received, MIB as u64);
        assert_eq!(result.saved_name, saved_name);
        assert_eq!(
            tokio::fs::read(host.downloads.join(&result.saved_name))
                .await
                .unwrap(),
            body
        );
    }
    assert_eq!(names(&host.downloads), ["a (1).txt", "a.txt"]);
    assert!(host.audit_events().is_empty());
}

#[tokio::test]
async fn reset_keeps_partial_without_status_and_resume_audits_the_whole_file_once() {
    let host = WireHost::start_with(WireOptions {
        audit: true,
        ..Default::default()
    })
    .await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x26; MIB];
    let mut header = header(&host, ID, &body).await;
    let mut stream = partial_upload(&host, &client, &header, &body).await;
    stream.reset(0u32.into()).unwrap();
    let result = wait_partial(&mut client, ID).await;
    assert_state(&result, ID, State::Partial, FileError::None);
    let length = std::fs::metadata(host.downloads.join(format!(".rs-{ID}.part")))
        .unwrap()
        .len();
    assert_eq!(result.received, length);
    assert_eq!(length, PREFIX as u64);
    client
        .expect_none_matching(Duration::from_millis(100), |msg| {
            matches!(msg, pb::envelope::Msg::FileStatus(_))
        })
        .await;
    assert!(host
        .audit_events()
        .iter()
        .all(|event| event.event != "file"));
    header.offset = result.received;
    upload(&client, &header, &body[header.offset as usize..]).await;
    assert_state(
        &status(&mut client).await,
        ID,
        State::Completed,
        FileError::None,
    );
    assert_eq!(
        tokio::fs::read(host.downloads.join("a.txt")).await.unwrap(),
        body
    );
    let events: Vec<_> = host
        .audit_events()
        .into_iter()
        .filter(|event| event.event == "file")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].detail,
        Some(serde_json::json!({"op": "upload", "files": 1, "bytes": MIB}))
    );
}

#[tokio::test]
async fn query_distinguishes_unknown_and_invalid_ids() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    assert_state(
        &query(&mut client, ID, 42).await,
        ID,
        State::Unknown,
        FileError::None,
    );
    assert_state(
        &query(&mut client, "abc", 43).await,
        "abc",
        State::Failed,
        FileError::Protocol,
    );
}

#[tokio::test]
async fn cancel_wakes_a_stalled_upload_removes_partials_and_does_not_audit() {
    let host = WireHost::start_with(WireOptions {
        audit: true,
        ..Default::default()
    })
    .await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x41; MIB];
    let header = header(&host, ID, &body).await;
    let mut stream = partial_upload(&host, &client, &header, &body).await;
    let receiving = query(&mut client, ID, 10).await;
    assert_state(&receiving, ID, State::Receiving, FileError::None);
    assert_eq!(receiving.received, PREFIX as u64);
    assert_state(
        &cancel(&mut client, ID).await,
        ID,
        State::Cancelled,
        FileError::None,
    );
    stopped(&mut stream, file_ctl::STREAM_CANCELLED).await;
    assert!(names(&host.downloads).is_empty());
    assert!(host
        .audit_events()
        .iter()
        .all(|event| event.event != "file"));
}

#[tokio::test]
async fn cancel_after_reset_removes_both_sidecars() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x42; MIB];
    let header = header(&host, ID, &body).await;
    let mut stream = partial_upload(&host, &client, &header, &body).await;
    stream.reset(0u32.into()).unwrap();
    assert_state(
        &wait_partial(&mut client, ID).await,
        ID,
        State::Partial,
        FileError::None,
    );
    assert_eq!(names(&host.downloads).len(), 2);
    assert_state(
        &cancel(&mut client, ID).await,
        ID,
        State::Cancelled,
        FileError::None,
    );
    assert!(names(&host.downloads).is_empty());
}

#[tokio::test]
async fn sha256_failure_reports_error_and_does_not_audit() {
    let host = WireHost::start_with(WireOptions {
        audit: true,
        ..Default::default()
    })
    .await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x43; MIB];
    let mut header = header(&host, ID, &body).await;
    header.sha256 = vec![0x99; 32];
    upload(&client, &header, &body).await;
    assert_state(
        &status(&mut client).await,
        ID,
        State::Failed,
        FileError::Sha256Mismatch,
    );
    assert!(names(&host.downloads).is_empty());
    assert!(host
        .audit_events()
        .iter()
        .all(|event| event.event != "file"));
}

#[tokio::test]
async fn disabled_upload_direction_stops_stream_without_files() {
    for disabled_all in [false, true] {
        let mut options = WireOptions::default();
        options.config.files.enabled = !disabled_all;
        options.config.files.from_phone = disabled_all;
        let host = WireHost::start_with(options).await;
        let mut client = host.connect().await;
        client.hello().await;
        let mut stream = client.open_typed(0x04).await;
        let _ = stream.write_all(&[0]).await;
        stopped(&mut stream, file_ctl::STREAM_DISABLED).await;
        assert!(names(&host.downloads).is_empty());
    }
}

#[tokio::test]
async fn query_reports_disabled_when_phone_uploads_are_off() {
    for (enabled, from_phone) in [(true, false), (false, true), (false, false)] {
        let mut options = WireOptions::default();
        options.config.files.enabled = enabled;
        options.config.files.from_phone = from_phone;
        let host = WireHost::start_with(options).await;
        let mut client = host.connect().await;
        client.hello().await;
        let result = query(&mut client, ID, 41).await;
        assert_state(&result, ID, State::Failed, FileError::Disabled);
        assert_eq!(result.request_id, 41);
        assert_eq!(result.received, 0);
        assert!(names(&host.downloads).is_empty());
    }
}

#[tokio::test]
async fn disabled_query_keeps_partial_and_reenabled_query_reports_it() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x49; MIB];
    let header = header(&host, ID, &body).await;
    let mut stream = partial_upload(&host, &client, &header, &body).await;
    stream.reset(2u32.into()).unwrap();
    let result = wait_partial(&mut client, ID).await;
    assert_state(&result, ID, State::Partial, FileError::None);
    assert_eq!(result.received, PREFIX as u64);

    host.config
        .update(&serde_json::json!({"files": {"from_phone": false}}))
        .unwrap();
    let result = query(&mut client, ID, 42).await;
    assert_state(&result, ID, State::Failed, FileError::Disabled);
    assert_eq!(result.received, 0);
    assert_eq!(
        partial_received(&host.downloads, ID).unwrap(),
        Some(PREFIX as u64)
    );

    host.config
        .update(&serde_json::json!({"files": {"from_phone": true}}))
        .unwrap();
    let result = query(&mut client, ID, 43).await;
    assert_state(&result, ID, State::Partial, FileError::None);
    assert_eq!(result.received, PREFIX as u64);
}

#[tokio::test]
async fn disabled_query_wins_over_viewer() {
    let mut options = WireOptions::default();
    options.options.view_only = true;
    options.config.files.from_phone = false;
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    client.hello().await;
    assert_state(
        &query(&mut client, ID, 44).await,
        ID,
        State::Failed,
        FileError::Disabled,
    );
}

#[tokio::test]
async fn configured_size_limit_reports_too_large_with_transfer_id() {
    let mut options = WireOptions::default();
    options.config.files.max_file_mb = 1;
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x44; 2 * MIB];
    let header = header(&host, ID, &body).await;
    let mut stream = open(&client, &header).await;
    assert_state(
        &status(&mut client).await,
        ID,
        State::Failed,
        FileError::TooLarge,
    );
    stopped(&mut stream, file_ctl::STREAM_REJECTED).await;
    assert!(names(&host.downloads).is_empty());
}

struct TestDir(std::path::PathBuf);
impl TestDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-filectl-{label}-{}-{}",
            std::process::id(),
            rsp_core::clock::unix_micros()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn configured_destination_is_created_and_used() {
    let dir = TestDir::new("destination");
    let dest = dir.0.join("new-downloads");
    let mut options = WireOptions::default();
    options.config.files.dest_dir = dest.to_string_lossy().into_owned();
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = b"configured destination";
    let header = header(&host, ID, body).await;
    upload(&client, &header, body).await;
    assert_state(
        &status(&mut client).await,
        ID,
        State::Completed,
        FileError::None,
    );
    assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), body);
    assert!(names(&host.downloads).is_empty());
}

#[tokio::test]
async fn per_header_destination_missing_dir_is_not_found() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let mut header = header(&host, ID, b"body").await;
    header.dest_dir = host.state_dir.join("other").to_string_lossy().into_owned();
    let mut stream = open(&client, &header).await;
    assert_state(
        &status(&mut client).await,
        ID,
        State::Failed,
        FileError::NotFound,
    );
    stopped(&mut stream, file_ctl::STREAM_REJECTED).await;
    assert!(names(&host.downloads).is_empty());
}

#[tokio::test]
async fn per_header_destination_existing_dir_receives_upload() {
    let dir = TestDir::new("header-destination");
    let mut options = WireOptions::default();
    options.config.files.browse_roots = vec![dir.0.to_string_lossy().into_owned()];
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = b"per-header destination";
    let mut header = header(&host, ID, body).await;
    header.dest_dir = dir.0.to_string_lossy().into_owned();
    upload(&client, &header, body).await;
    let result = status(&mut client).await;
    assert_state(&result, ID, State::Completed, FileError::None);
    assert_eq!(result.received, body.len() as u64);
    assert_eq!(result.saved_name, "a.txt");
    assert_eq!(std::fs::read(dir.0.join("a.txt")).unwrap(), body);
    assert_eq!(names(&dir.0), ["a.txt"]);
    assert!(names(&host.downloads).is_empty());
}

#[tokio::test]
async fn per_header_destination_outside_browse_roots_is_rejected() {
    let root = TestDir::new("header-root");
    let outside = TestDir::new("header-outside");
    let mut options = WireOptions::default();
    options.config.files.browse_roots = vec![root.0.to_string_lossy().into_owned()];
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    client.hello().await;
    let mut header = header(&host, ID, b"body").await;
    header.dest_dir = outside.0.to_string_lossy().into_owned();
    let mut stream = open(&client, &header).await;
    assert_state(
        &status(&mut client).await,
        ID,
        State::Failed,
        FileError::OutsideRoot,
    );
    stopped(&mut stream, file_ctl::STREAM_REJECTED).await;
    assert!(names(&outside.0).is_empty());
    assert!(names(&host.downloads).is_empty());
}

#[tokio::test]
async fn query_from_new_connection_stops_old_receive_and_resumes_within_two_seconds() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    let body = vec![0x45; MIB];
    let mut header = header(&host, ID, &body).await;
    let _old = partial_upload(&host, &a, &header, &body).await;
    let mut b = second_connection(&host, Dev::A).await;
    let result = timeout(Duration::from_secs(2), query(&mut b, ID, 23))
        .await
        .unwrap();
    assert_state(&result, ID, State::Partial, FileError::None);
    assert_eq!(result.received, PREFIX as u64);
    header.offset = result.received;
    upload(&b, &header, &body[header.offset as usize..]).await;
    assert_state(&status(&mut b).await, ID, State::Completed, FileError::None);
    assert_eq!(std::fs::read(host.downloads.join("a.txt")).unwrap(), body);
    a.send(pb::Ping { t1_us: 23 }).await;
    a.next_matching(|msg| match msg {
        pb::envelope::Msg::Pong(p) if p.t1_us == 23 => Some(()),
        _ => None,
    })
    .await;
}

#[tokio::test]
async fn duplicate_id_on_new_connection_is_busy_then_can_retry() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    let body = vec![0x46; MIB];
    let mut header = header(&host, ID, &body).await;
    let _old = partial_upload(&host, &a, &header, &body).await;
    let mut b = second_connection(&host, Dev::A).await;
    header.offset = PREFIX as u64;
    let mut duplicate = open(&b, &header).await;
    stopped(&mut duplicate, file_ctl::STREAM_BUSY).await;
    assert_state(&status(&mut b).await, ID, State::Failed, FileError::Busy);
    // A query acts as a completion barrier; no fixed sleep is needed for retry.
    let result = timeout(Duration::from_secs(2), query(&mut b, ID, 24))
        .await
        .unwrap();
    assert_state(&result, ID, State::Partial, FileError::None);
    upload(&b, &header, &body[PREFIX..]).await;
    assert_state(&status(&mut b).await, ID, State::Completed, FileError::None);
    assert_eq!(std::fs::read(host.downloads.join("a.txt")).unwrap(), body);
}

#[tokio::test]
async fn another_device_cannot_cancel_active_upload() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    let body = vec![0x47; MIB];
    let header = header(&host, ID, &body).await;
    let mut stream = partial_upload(&host, &a, &header, &body).await;
    let mut b = second_connection(&host, Dev::B).await;
    b.send(pb::FileCancel {
        transfer_id: ID.to_owned(),
    })
    .await;
    b.send(pb::Ping { t1_us: 25 }).await;
    b.next_skipping_session_state("Pong", |msg| match msg {
        pb::envelope::Msg::Pong(p) if p.t1_us == 25 => Some(()),
        _ => None,
    })
    .await;
    assert_state(
        &query(&mut a, ID, 25).await,
        ID,
        State::Receiving,
        FileError::None,
    );
    stream.write_all(&body[PREFIX..]).await.unwrap();
    stream.finish().unwrap();
    assert_state(&status(&mut a).await, ID, State::Completed, FileError::None);
}

#[tokio::test]
async fn viewer_query_and_cancel_are_denied_and_invalid_cancel_is_ignored() {
    let mut options = WireOptions::default();
    options.options.view_only = true;
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    client.hello().await;
    assert_state(
        &query(&mut client, ID, 31).await,
        ID,
        State::Failed,
        FileError::NotAllowed,
    );
    assert_state(
        &cancel(&mut client, ID).await,
        ID,
        State::Failed,
        FileError::NotAllowed,
    );

    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    client
        .send(pb::FileCancel {
            transfer_id: "abc".to_owned(),
        })
        .await;
    client.send(pb::Ping { t1_us: 32 }).await;
    client
        .next_skipping_session_state("Pong", |msg| match msg {
            pb::envelope::Msg::Pong(p) if p.t1_us == 32 => Some(()),
            _ => None,
        })
        .await;
}

#[tokio::test]
async fn resume_mismatch_returns_current_partial_length() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x48; MIB];
    let mut header = header(&host, ID, &body).await;
    let mut stream = partial_upload(&host, &client, &header, &body).await;
    stream.reset(0u32.into()).unwrap();
    assert_state(
        &wait_partial(&mut client, ID).await,
        ID,
        State::Partial,
        FileError::None,
    );
    header.offset = (PREFIX + 1) as u64;
    let _stream = open(&client, &header).await;
    let result = status(&mut client).await;
    assert_state(&result, ID, State::Failed, FileError::ResumeMismatch);
    assert_eq!(result.received, PREFIX as u64);
}

#[tokio::test]
async fn controller_survives_eleven_minutes_and_peer_status_does_not_audit() {
    let host = WireHost::start_with(WireOptions {
        audit: true,
        ..Default::default()
    })
    .await;
    let mut client = host.connect().await;
    client.hello().await;
    for state in [State::Completed, State::Failed, State::Partial] {
        client
            .send(FileStatus {
                transfer_id: ID.to_owned(),
                state: state as i32,
                ..Default::default()
            })
            .await;
    }
    host.clock.skip(Duration::from_secs(11 * 60));
    client
        .expect_none_matching(Duration::from_millis(1200), |msg| {
            matches!(
                msg,
                pb::envelope::Msg::Goodbye(_) | pb::envelope::Msg::FileStatus(_)
            )
        })
        .await;
    client.send(pb::Ping { t1_us: 33 }).await;
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::Pong(p) if p.t1_us == 33 => Some(()),
            _ => None,
        })
        .await;
    assert_eq!(host.conns().len(), 1);
    assert!(host
        .audit_events()
        .iter()
        .all(|event| event.event != "file"));
}

#[tokio::test]
async fn startup_removes_eight_day_partials_and_keeps_one_day_partials() {
    let state = TestDir::new("cleanup");
    let downloads = state.0.join("downloads");
    std::fs::create_dir_all(&downloads).unwrap();
    for (id, days) in [(ID, 8), (ID2, 1)] {
        for extension in ["part", "json"] {
            let path = downloads.join(format!(".rs-{id}.{extension}"));
            let file = std::fs::File::create(path).unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
                .unwrap();
        }
    }
    let host = WireHost::start_with(WireOptions {
        state_dir: Some(state.0.clone()),
        ..Default::default()
    })
    .await;
    host.wait_until(|| {
        !downloads.join(format!(".rs-{ID}.part")).exists()
            && !downloads.join(format!(".rs-{ID}.json")).exists()
    })
    .await;
    assert_eq!(
        names(&downloads),
        [format!(".rs-{ID2}.json"), format!(".rs-{ID2}.part")]
    );
}

#[tokio::test]
async fn query_after_completed_upload_reports_completed_with_saved_name() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    let body = vec![0x51; MIB];
    for (id, saved_name) in [(ID, "a.txt"), (ID2, "a (1).txt")] {
        let header = header(&host, id, &body).await;
        upload(&a, &header, &body).await;
        let result = status(&mut a).await;
        assert_state(&result, id, State::Completed, FileError::None);
        assert_eq!(result.saved_name, saved_name);
    }

    let mut b = second_connection(&host, Dev::A).await;
    let result = query(&mut b, ID2, 31).await;
    assert_state(&result, ID2, State::Completed, FileError::None);
    assert_eq!(result.received, MIB as u64);
    assert_eq!(result.saved_name, "a (1).txt");
    let result = query(&mut b, ID, 32).await;
    assert_state(&result, ID, State::Completed, FileError::None);
    assert_eq!(result.received, MIB as u64);
    assert_eq!(result.saved_name, "a.txt");

    let result = query(&mut a, ID, 33).await;
    assert_state(&result, ID, State::Completed, FileError::None);
    assert_eq!(result.received, MIB as u64);
    assert_eq!(result.saved_name, "a.txt");
    assert_eq!(names(&host.downloads), ["a (1).txt", "a.txt"]);
}

#[tokio::test]
async fn completed_query_is_per_device_and_expires() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    let body = vec![0x52; MIB];
    let header = header(&host, ID, &body).await;
    upload(&a, &header, &body).await;
    assert_state(&status(&mut a).await, ID, State::Completed, FileError::None);

    let mut b = second_connection(&host, Dev::B).await;
    assert_state(
        &query(&mut b, ID, 35).await,
        ID,
        State::Unknown,
        FileError::None,
    );

    host.clock.skip(Duration::from_secs(24 * 60 * 60 - 60));
    let mut before_expiry = second_connection(&host, Dev::A).await;
    let result = query(&mut before_expiry, ID, 36).await;
    assert_state(&result, ID, State::Completed, FileError::None);
    assert_eq!(result.received, MIB as u64);
    assert_eq!(result.saved_name, "a.txt");

    host.clock.skip(Duration::from_secs(120));
    let mut after_expiry = second_connection(&host, Dev::A).await;
    assert_state(
        &query(&mut after_expiry, ID, 37).await,
        ID,
        State::Unknown,
        FileError::None,
    );
}

#[tokio::test]
async fn reupload_with_same_id_forgets_completed_record() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let body1 = vec![0x52; MIB];
    let first_header = header(&host, ID, &body1).await;
    upload(&client, &first_header, &body1).await;
    let result = status(&mut client).await;
    assert_state(&result, ID, State::Completed, FileError::None);
    assert_eq!(result.saved_name, "a.txt");

    let body2 = vec![0x53; MIB];
    let mut second_header = header(&host, ID, &body2).await;
    let mut stream = partial_upload(&host, &client, &second_header, &body2).await;
    stream.reset(0u32.into()).unwrap();
    let result = wait_partial(&mut client, ID).await;
    assert_state(&result, ID, State::Partial, FileError::None);
    assert_eq!(result.received, PREFIX as u64);

    second_header.offset = PREFIX as u64;
    upload(&client, &second_header, &body2[PREFIX..]).await;
    let result = status(&mut client).await;
    assert_state(&result, ID, State::Completed, FileError::None);
    assert_eq!(result.received, MIB as u64);
    assert_eq!(result.saved_name, "a (1).txt");

    let result = query(&mut client, ID, 34).await;
    assert_state(&result, ID, State::Completed, FileError::None);
    assert_eq!(result.received, MIB as u64);
    assert_eq!(result.saved_name, "a (1).txt");
}

#[tokio::test]
async fn failed_and_cancelled_uploads_are_not_recorded() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x54; MIB];
    let mut bad_header = header(&host, ID, &body).await;
    bad_header.sha256 = vec![0x99; 32];
    upload(&client, &bad_header, &body).await;
    assert_state(
        &status(&mut client).await,
        ID,
        State::Failed,
        FileError::Sha256Mismatch,
    );
    assert_state(
        &query(&mut client, ID, 38).await,
        ID,
        State::Unknown,
        FileError::None,
    );

    let body2 = vec![0x55; MIB];
    let header2 = header(&host, ID2, &body2).await;
    let mut stream = partial_upload(&host, &client, &header2, &body2).await;
    assert_state(
        &cancel(&mut client, ID2).await,
        ID2,
        State::Cancelled,
        FileError::None,
    );
    stopped(&mut stream, file_ctl::STREAM_CANCELLED).await;
    assert_state(
        &query(&mut client, ID2, 39).await,
        ID2,
        State::Unknown,
        FileError::None,
    );
}

#[tokio::test]
async fn disabled_query_wins_over_completed() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x56; MIB];
    let header = header(&host, ID, &body).await;
    upload(&client, &header, &body).await;
    assert_state(
        &status(&mut client).await,
        ID,
        State::Completed,
        FileError::None,
    );

    host.config
        .update(&serde_json::json!({"files": {"from_phone": false}}))
        .unwrap();
    assert_state(
        &query(&mut client, ID, 40).await,
        ID,
        State::Failed,
        FileError::Disabled,
    );

    host.config
        .update(&serde_json::json!({"files": {"from_phone": true}}))
        .unwrap();
    let result = query(&mut client, ID, 41).await;
    assert_state(&result, ID, State::Completed, FileError::None);
    assert_eq!(result.received, MIB as u64);
    assert_eq!(result.saved_name, "a.txt");
}

#[tokio::test]
async fn disabled_stream_with_header_reports_failed_status() {
    let mut options = WireOptions::default();
    options.config.files.from_phone = false;
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    client.hello().await;
    let header = header(&host, ID, b"one").await;
    let mut stream = open(&client, &header).await;
    stopped(&mut stream, file_ctl::STREAM_DISABLED).await;
    let result = status(&mut client).await;
    assert_state(&result, ID, State::Failed, FileError::Disabled);
    assert_eq!(result.request_id, 0);
    assert_eq!(result.received, 0);
    assert_eq!(result.saved_name, "");
    assert!(names(&host.downloads).is_empty());
}

#[tokio::test]
async fn viewer_stream_with_header_reports_not_allowed() {
    let mut options = WireOptions::default();
    options.options.view_only = true;
    let host = WireHost::start_with(options).await;
    let mut viewer = host.connect().await;
    let (_, auth) = viewer.hello().await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    let header = header(&host, ID, b"one").await;
    let mut stream = open(&viewer, &header).await;
    stopped(&mut stream, file_ctl::STREAM_NOT_ALLOWED).await;
    let result = viewer
        .next_matching(|message| match message {
            pb::envelope::Msg::FileStatus(status) => Some(status.clone()),
            _ => None,
        })
        .await;
    assert_state(&result, ID, State::Failed, FileError::NotAllowed);
}

#[tokio::test]
async fn fifth_stream_with_header_reports_busy() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let body = vec![0x61; MIB];
    let mut streams = Vec::new();
    for i in 1..=4 {
        let id = format!("{i:032x}");
        let header = header(&host, &id, &body).await;
        streams.push(partial_upload(&host, &client, &header, &body).await);
    }
    let fifth_id = format!("{:032x}", 5);
    let fifth_header = header(&host, &fifth_id, &body).await;
    let mut fifth = open(&client, &fifth_header).await;
    stopped(&mut fifth, file_ctl::STREAM_BUSY).await;
    let result = status(&mut client).await;
    assert_state(&result, &fifth_id, State::Failed, FileError::Busy);
    for stream in &mut streams {
        stream.reset(0u32.into()).unwrap();
    }
}

#[tokio::test]
async fn rejected_stream_without_valid_header_reports_nothing() {
    let mut options = WireOptions::default();
    options.config.files.from_phone = false;
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    client.hello().await;

    let mut first = client.open_typed(0x04).await;
    first.write_all(&[0]).await.unwrap();
    stopped(&mut first, file_ctl::STREAM_DISABLED).await;

    let mut second = client.open_typed(0x04).await;
    second.write_all(&[0xff, 0xff, 0xff, 0xff]).await.unwrap();
    stopped(&mut second, file_ctl::STREAM_DISABLED).await;

    let invalid_header = [
        0, 0, 0, 12, 0x0a, 3, b'a', b'b', b'c', 0x12, 5, b'a', b'.', b't', b'x', b't',
    ];
    let mut third = client.open_typed(0x04).await;
    third.write_all(&invalid_header).await.unwrap();
    stopped(&mut third, file_ctl::STREAM_DISABLED).await;

    client
        .expect_none_matching(Duration::from_millis(1500), |message| {
            matches!(message, pb::envelope::Msg::FileStatus(_))
        })
        .await;
    assert!(names(&host.downloads).is_empty());
}

#[tokio::test]
async fn pending_reject_cap_stops_extra_streams_without_reading() {
    let mut options = WireOptions::default();
    options.config.files.from_phone = false;
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    client.hello().await;

    let mut pending = Vec::new();
    for _ in 0..4 {
        let mut stream = client.open_typed(0x04).await;
        stream.write_all(&[0]).await.unwrap();
        pending.push(stream);
    }

    let first_header = header(&host, ID, b"one").await;
    let mut fifth = open(&client, &first_header).await;
    stopped(&mut fifth, file_ctl::STREAM_DISABLED).await;
    client
        .expect_none_matching(Duration::from_millis(1500), |message| {
            matches!(message, pb::envelope::Msg::FileStatus(_))
        })
        .await;
    for stream in &mut pending {
        stopped(stream, file_ctl::STREAM_DISABLED).await;
    }

    let header = header(&host, ID2, b"one").await;
    let mut sixth = open(&client, &header).await;
    stopped(&mut sixth, file_ctl::STREAM_DISABLED).await;
    let result = status(&mut client).await;
    assert_state(&result, ID2, State::Failed, FileError::Disabled);
}
