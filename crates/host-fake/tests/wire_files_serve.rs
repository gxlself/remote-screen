use host_core::{file_send, session::handlers::files};
use rsp_core::{
    file::{encode_stream_prefix, FileStreamReceiver, StreamEnd},
    file_ctl,
    pb::{self, file_status::State, FileError, FileHeader, FileListing, FileRequest, FileStatus},
};
use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::time::timeout;

mod wire_support;
use wire_support::{client_hello, Dev, WireClient, WireHost, WireOptions};

const MIB: usize = 1024 * 1024;
const ID: &str = "11111111111111111111111111111111";
const ID2: &str = "22222222222222222222222222222222";
const ID3: &str = "33333333333333333333333333333333";

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let dir = Self(std::env::temp_dir().join(format!(
            "rs-files-serve-{}-{}",
            std::process::id(),
            rsp_core::file::new_transfer_id().unwrap()
        )));
        std::fs::create_dir_all(&dir.0).unwrap();
        dir
    }

    fn options(&self) -> WireOptions {
        let mut options = WireOptions {
            audit: true,
            ..Default::default()
        };
        options.config.files.browse_roots = vec![self.0.to_string_lossy().into_owned()];
        options
    }

    fn file(&self, name: &str, size: usize) -> PathBuf {
        let path = self.0.join(name);
        std::fs::File::create(&path)
            .unwrap()
            .set_len(size as u64)
            .unwrap();
        path
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn request(path: &Path, id: &str, request_id: u32) -> FileRequest {
    FileRequest {
        request_id,
        path: path.to_string_lossy().into_owned(),
        transfer_id: id.to_owned(),
        ..Default::default()
    }
}

async fn listing(client: &mut WireClient, path: &str, offset: u32, id: u32) -> FileListing {
    client
        .send(pb::FileList {
            request_id: id,
            path: path.to_owned(),
            offset,
        })
        .await;
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::FileListing(listing) if listing.request_id == id => {
                Some(listing.clone())
            }
            _ => None,
        })
        .await
}

async fn status(client: &mut WireClient) -> FileStatus {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::FileStatus(status) => Some(status.clone()),
            _ => None,
        })
        .await
}

fn assert_status(status: &FileStatus, id: &str, request_id: u32, state: State, error: FileError) {
    assert_eq!(status.transfer_id, id);
    assert_eq!(status.request_id, request_id);
    assert_eq!(status.state, state as i32);
    assert_eq!(status.error, error as i32);
}

async fn ping(client: &mut WireClient) {
    client.send(pb::Ping { t1_us: 42 }).await;
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::Pong(p) if p.t1_us == 42 => Some(()),
            _ => None,
        })
        .await;
}

fn downloads(host: &WireHost) -> Vec<host_core::audit::HostAuditRecord> {
    host.audit_events()
        .into_iter()
        .filter(|event| {
            event.event == "file" && event.detail.as_ref().is_some_and(|d| d["op"] == "download")
        })
        .collect()
}

struct Incoming {
    recv: quinn::RecvStream,
    header: FileHeader,
    parser: FileStreamReceiver,
}
impl Incoming {
    async fn accept(client: &WireClient) -> Self {
        let (kind, mut recv) = client.accept_typed().await;
        assert_eq!(kind, 0x04);
        let mut length = [0; 4];
        recv.read_exact(&mut length).await.unwrap();
        let size = u32::from_be_bytes(length) as usize;
        assert!(size <= 4096);
        let mut prefix = vec![0; size + 4];
        prefix[..4].copy_from_slice(&length);
        recv.read_exact(&mut prefix[4..]).await.unwrap();
        let mut parser = FileStreamReceiver::new();
        let header = parser.feed(&prefix).unwrap().header.unwrap();
        assert!(header.dest_dir.is_empty());
        Self {
            recv,
            header,
            parser,
        }
    }

    async fn prefix(&mut self, count: usize) -> Vec<u8> {
        let mut bytes = vec![0; count];
        self.recv.read_exact(&mut bytes).await.unwrap();
        assert_eq!(self.parser.feed(&bytes).unwrap().data.unwrap().bytes, bytes);
        bytes
    }

    async fn finish(mut self) -> Vec<u8> {
        let remaining = (self.header.size - self.header.offset) as usize;
        let bytes = timeout(
            Duration::from_secs(10),
            self.recv.read_to_end(remaining + 1),
        )
        .await
        .unwrap()
        .unwrap();
        let fed = self.parser.feed(&bytes).unwrap();
        if !bytes.is_empty() {
            assert_eq!(fed.data.unwrap().bytes, bytes);
        }
        assert_eq!(
            self.parser.finish().unwrap(),
            StreamEnd::Complete {
                header: self.header
            }
        );
        bytes
    }
}

#[tokio::test]
async fn default_root_sorts_entries_counts_children_and_reports_parent() {
    let root = TestDir::new();
    std::fs::create_dir(root.0.join("z-dir")).unwrap();
    std::fs::write(root.0.join("z-dir/child"), b"x").unwrap();
    root.file("b.bin", 2);
    root.file("A.bin", 1);
    let host = WireHost::start_with(root.options()).await;
    let mut client = host.connect().await;
    client.hello().await;
    let result = listing(&mut client, "", 0, 1).await;
    assert_eq!(result.error, FileError::None as i32);
    assert_eq!(Path::new(&result.path), root.0.canonicalize().unwrap());
    assert!(result.parent.is_empty());
    assert_eq!(result.next_offset, 0);
    assert_eq!(
        result
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["z-dir", "A.bin", "b.bin"]
    );
    assert_eq!(result.entries[0].kind, pb::file_entry::Kind::Dir as i32);
    assert_eq!(result.entries[0].child_count, 1);
    let child = listing(&mut client, root.0.join("z-dir").to_str().unwrap(), 0, 2).await;
    assert_eq!(child.parent, result.path);
}

#[tokio::test]
async fn listing_pages_2500_entries_without_duplicates() {
    let root = TestDir::new();
    for index in 0..2500 {
        root.file(&format!("{index:04}.bin"), 0);
    }
    let host = WireHost::start_with(root.options()).await;
    let mut client = host.connect().await;
    client.hello().await;
    let mut names = HashSet::new();
    for (id, offset, count, next) in [(1, 0, 1000, 1000), (2, 1000, 1000, 2000), (3, 2000, 500, 0)]
    {
        let result = listing(&mut client, "", offset, id).await;
        assert_eq!(result.error, FileError::None as i32);
        assert_eq!(result.entries.len(), count);
        assert_eq!(result.next_offset, next);
        for entry in result.entries {
            assert!(names.insert(entry.name));
        }
    }
    assert_eq!(names.len(), 2500);
    assert!(listing(&mut client, "", 2500, 4).await.entries.is_empty());
}

#[tokio::test]
async fn listing_errors_keep_the_original_path_and_configuration_is_live() {
    let root = TestDir::new();
    let outside = TestDir::new();
    let file = root.file("file.bin", 1);
    let host = WireHost::start_with(root.options()).await;
    let mut client = host.connect().await;
    client.hello().await;
    for (id, path, error) in [
        (1, outside.0.clone(), FileError::OutsideRoot),
        (2, root.0.join("missing"), FileError::NotFound),
        (3, file, FileError::NotADir),
        (4, host.state_dir.clone(), FileError::OutsideRoot),
    ] {
        let result = listing(&mut client, path.to_str().unwrap(), 0, id).await;
        assert_eq!(result.error, error as i32);
        assert_eq!(result.path, path.to_str().unwrap());
        assert!(result.entries.is_empty());
    }
    host.config
        .update(&serde_json::json!({"files": {"browse": false}}))
        .unwrap();
    assert_eq!(
        listing(&mut client, "", 0, 5).await.error,
        FileError::Disabled as i32
    );
    assert_eq!(
        listing(&mut client, "bad\0path", 0, 6).await.error,
        FileError::Protocol as i32
    );
    host.config
        .update(&serde_json::json!({"files": {"browse": true,
        "browse_roots": [outside.0.to_str().unwrap()]}}))
        .unwrap();
    assert_eq!(
        listing(&mut client, "", 0, 7).await.path,
        outside.0.canonicalize().unwrap().to_str().unwrap()
    );
}

#[tokio::test]
async fn viewer_cannot_list_or_download_and_produces_no_download_audit() {
    let root = TestDir::new();
    let path = root.file("private.bin", 1);
    let mut options = root.options();
    options.options.view_only = true;
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    let (_, auth) = client.hello().await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    assert_eq!(
        listing(&mut client, "", 0, 1).await.error,
        FileError::NotAllowed as i32
    );
    client.send(request(&path, ID, 2)).await;
    assert_status(
        &status(&mut client).await,
        ID,
        2,
        State::Failed,
        FileError::NotAllowed,
    );
    assert!(
        timeout(Duration::from_millis(150), client.conn.accept_uni())
            .await
            .is_err()
    );
    assert!(downloads(&host).is_empty());
}

#[tokio::test]
async fn download_header_body_and_audit_match_the_source() {
    let root = TestDir::new();
    let body: Vec<_> = (0..4 * MIB)
        .map(|i| ((i * 31 + i / 127) % 251) as u8)
        .collect();
    let path = root.0.join("f.bin");
    std::fs::write(&path, &body).unwrap();
    let expected = file_send::prepare(&path, None).await.unwrap();
    let host = WireHost::start_with(root.options()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(request(&path, "", 1)).await;
    let incoming = Incoming::accept(&client).await;
    assert_eq!(incoming.header.size, body.len() as u64);
    assert_eq!(incoming.header.sha256, expected.sha256);
    assert_eq!(incoming.header.mtime_us, expected.mtime_us);
    assert_eq!(incoming.header.offset, 0);
    host.clock.skip(Duration::from_secs(2));
    assert_eq!(incoming.finish().await, body);
    host.wait_until(|| downloads(&host).len() == 1).await;
    let event = &downloads(&host)[0];
    assert_eq!(event.duration_s, Some(2));
    assert_eq!(
        event.detail,
        Some(serde_json::json!({"op": "download", "files": 1, "bytes": body.len()}))
    );
    client
        .expect_none_matching(Duration::from_millis(100), |m| {
            matches!(m, pb::envelope::Msg::FileStatus(_))
        })
        .await;
}

async fn resume_test(mismatch: bool) {
    let root = TestDir::new();
    let path = root.file("resume.bin", 16 * MIB);
    let host = WireHost::start_with(root.options()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(request(&path, ID, 1)).await;
    let mut first = Incoming::accept(&client).await;
    let prefix = first.prefix(MIB).await;
    let header = first.header.clone();
    first.recv.stop(file_ctl::STREAM_PAUSED.into()).unwrap();
    ping(&mut client).await;
    assert!(downloads(&host).is_empty());
    client
        .send(FileRequest {
            offset: MIB as u64,
            expect_size: header.size,
            expect_mtime_us: header.mtime_us + u64::from(mismatch),
            expect_sha256: header.sha256,
            ..request(&path, ID, 2)
        })
        .await;
    let resumed = Incoming::accept(&client).await;
    assert_eq!(resumed.header.transfer_id, ID);
    assert_eq!(resumed.header.offset, if mismatch { 0 } else { MIB as u64 });
    let mut bytes = if mismatch { Vec::new() } else { prefix };
    bytes.extend_from_slice(&resumed.finish().await);
    assert_eq!(bytes, std::fs::read(&path).unwrap());
    host.wait_until(|| downloads(&host).len() == 1).await;
    assert_eq!(
        downloads(&host)[0].detail.as_ref().unwrap()["bytes"],
        16 * MIB
    );
    client
        .expect_none_matching(Duration::from_millis(100), |m| {
            matches!(m, pb::envelope::Msg::FileStatus(_))
        })
        .await;
}

#[tokio::test]
async fn paused_download_resumes_from_confirmed_offset() {
    resume_test(false).await;
}

#[tokio::test]
async fn changed_resume_metadata_restarts_from_zero() {
    resume_test(true).await;
}

#[tokio::test]
async fn source_append_fails_without_audit_and_invalidates_preparation() {
    let root = TestDir::new();
    let path = root.file("change.bin", 16 * MIB);
    let host = WireHost::start_with(root.options()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(request(&path, ID, 1)).await;
    let first = Incoming::accept(&client).await;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&[1])
        .unwrap();
    let (_, result) = tokio::join!(
        async {
            let mut recv = first.recv;
            let _ = recv.read_to_end(17 * MIB).await;
        },
        status(&mut client),
    );
    assert_status(&result, ID, 1, State::Failed, FileError::SourceChanged);
    assert!(downloads(&host).is_empty());
    client.send(request(&path, ID2, 2)).await;
    let second = Incoming::accept(&client).await;
    assert_eq!(second.header.size, (16 * MIB + 1) as u64);
    assert_eq!(second.finish().await.last(), Some(&1));
}

#[tokio::test]
async fn third_concurrent_request_is_busy_and_first_two_complete() {
    let root = TestDir::new();
    let path = root.file("parallel.bin", 16 * MIB);
    let host = WireHost::start_with(root.options()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(request(&path, ID, 1)).await;
    let first = Incoming::accept(&client).await;
    client.send(request(&path, ID2, 2)).await;
    let second = Incoming::accept(&client).await;
    client.send(request(&path, ID3, 3)).await;
    assert_status(
        &status(&mut client).await,
        ID3,
        3,
        State::Failed,
        FileError::Busy,
    );
    let (a, b) = tokio::join!(first.finish(), second.finish());
    assert_eq!(a.len(), 16 * MIB);
    assert_eq!(a, b);
    host.wait_until(|| downloads(&host).len() == 2).await;
}

#[tokio::test]
async fn cancellation_interrupts_a_blocked_send_and_releases_the_slot() {
    let root = TestDir::new();
    let path = root.file("cancel.bin", 16 * MIB);
    let host = WireHost::start_with(root.options()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(request(&path, ID, 1)).await;
    let first = Incoming::accept(&client).await;
    client
        .send(pb::FileCancel {
            transfer_id: ID.to_owned(),
        })
        .await;
    assert_status(
        &status(&mut client).await,
        ID,
        1,
        State::Cancelled,
        FileError::None,
    );
    let mut recv = first.recv;
    let ended = timeout(Duration::from_secs(3), recv.read_to_end(17 * MIB))
        .await
        .unwrap();
    assert!(ended.is_err() || ended.unwrap().len() < 16 * MIB);
    assert!(downloads(&host).is_empty());
    client.send(request(&path, ID2, 2)).await;
    assert_eq!(
        Incoming::accept(&client).await.finish().await.len(),
        16 * MIB
    );
}

#[tokio::test]
async fn disabled_download_and_malformed_requests_fail_before_opening_streams() {
    let root = TestDir::new();
    let path = root.file("disabled.bin", 1);
    let mut options = root.options();
    options.config.files.to_phone = false;
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(request(&path, ID, 1)).await;
    assert_status(
        &status(&mut client).await,
        ID,
        1,
        State::Failed,
        FileError::Disabled,
    );
    client.send(request(Path::new(""), ID, 2)).await;
    assert_status(
        &status(&mut client).await,
        ID,
        2,
        State::Failed,
        FileError::Protocol,
    );
    assert!(
        timeout(Duration::from_millis(100), client.conn.accept_uni())
            .await
            .is_err()
    );
    assert!(downloads(&host).is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn outside_symlink_and_state_directory_cannot_be_downloaded() {
    let root = TestDir::new();
    let outside = TestDir::new();
    let path = outside.file("private.bin", 1);
    let link = root.0.join("link.bin");
    std::os::unix::fs::symlink(path, &link).unwrap();
    let host = WireHost::start_with(root.options()).await;
    // Even when explicitly configured as a root, state_dir remains denied.
    host.config
        .update(&serde_json::json!({"files": {"browse_roots": [
            root.0.to_str().unwrap(), host.state_dir.to_str().unwrap()
        ]}}))
        .unwrap();
    let secret = host.state_dir.join("secret.bin");
    std::fs::write(&secret, b"secret").unwrap();
    let mut client = host.connect().await;
    client.hello().await;
    for (id, path) in [(1, link), (2, secret)] {
        client.send(request(&path, ID, id)).await;
        assert_status(
            &status(&mut client).await,
            ID,
            id,
            State::Failed,
            FileError::OutsideRoot,
        );
    }
    assert!(
        timeout(Duration::from_millis(100), client.conn.accept_uni())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn upload_destination_is_resolved_and_rejections_use_control_errors() {
    let root = TestDir::new();
    let outside = TestDir::new();
    let sub = root.0.join("sub");
    std::fs::create_dir(&sub).unwrap();
    let path = root.file("source.bin", 3);
    let host = WireHost::start_with(root.options()).await;
    let mut client = host.connect().await;
    client.hello().await;
    let prepared = file_send::prepare(&path, Some(ID.to_owned()))
        .await
        .unwrap();
    let mut header =
        rsp_core::file::build_header(ID, "upload.bin", 3, 0, prepared.sha256, prepared.mtime_us)
            .unwrap();
    header.dest_dir = sub.to_string_lossy().into_owned();
    let mut stream = client.open_typed(0x04).await;
    stream
        .write_all(&encode_stream_prefix(&header).unwrap())
        .await
        .unwrap();
    stream.write_all(&[0; 3]).await.unwrap();
    stream.finish().unwrap();
    assert_status(
        &status(&mut client).await,
        ID,
        0,
        State::Completed,
        FileError::None,
    );
    assert_eq!(std::fs::read(sub.join("upload.bin")).unwrap(), [0; 3]);
    assert!(!host.downloads.join("upload.bin").exists());
    for (id, destination, disabled, code, error) in [
        (
            ID2,
            outside.0.clone(),
            false,
            file_ctl::STREAM_REJECTED,
            FileError::OutsideRoot,
        ),
        (
            ID3,
            sub.clone(),
            true,
            file_ctl::STREAM_DISABLED,
            FileError::Disabled,
        ),
    ] {
        host.config
            .update(&serde_json::json!({"files": {"browse": !disabled}}))
            .unwrap();
        header.transfer_id = id.to_owned();
        header.dest_dir = destination.to_string_lossy().into_owned();
        let mut stream = client.open_typed(0x04).await;
        stream
            .write_all(&encode_stream_prefix(&header).unwrap())
            .await
            .unwrap();
        assert_status(&status(&mut client).await, id, 0, State::Failed, error);
        assert_eq!(
            timeout(Duration::from_secs(3), stream.stopped())
                .await
                .unwrap()
                .unwrap(),
            Some(code.into())
        );
    }
}

#[tokio::test]
async fn a_new_connection_replaces_an_active_send_with_the_same_id() {
    let root = TestDir::new();
    let path = root.file("replace.bin", 16 * MIB);
    let host = WireHost::start_with(root.options()).await;
    let mut a = host.connect().await;
    a.hello().await;
    a.send(request(&path, ID, 1)).await;
    let first = Incoming::accept(&a).await;
    let mut b = host.connect_as(Dev::B, 0).await;
    let mut hello = client_hello();
    hello.intent = pb::Intent::Files as i32;
    hello.features |= rsp_core::features::FILE_BROWSE;
    hello.required_features = rsp_core::features::FILE_BROWSE;
    // FILES connections receive AuthResult without a roster SessionState.
    b.send(hello).await;
    let auth = b
        .next_matching(|msg| match msg {
            pb::envelope::Msg::AuthResult(auth) => Some(auth.clone()),
            _ => None,
        })
        .await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    b.send(request(&path, ID, 2)).await;
    let second = Incoming::accept(&b).await;
    assert_eq!(second.header.transfer_id, ID);
    assert_eq!(second.finish().await.len(), 16 * MIB);
    let mut recv = first.recv;
    let ended = timeout(Duration::from_secs(3), recv.read_to_end(17 * MIB))
        .await
        .unwrap();
    assert!(ended.is_err() || ended.unwrap().len() < 16 * MIB);
    host.wait_until(|| downloads(&host).len() == 1).await;
}

#[tokio::test]
async fn local_send_shares_request_limits_and_can_be_cancelled() {
    let root = TestDir::new();
    let path = root.file("local.bin", 16 * MIB);
    let host = WireHost::start_with(root.options()).await;
    let mut client = host.connect().await;
    client.hello().await;
    let mgr = host.mgr.clone();
    let key = host.conns()[0].key;
    let local_path = path.clone();
    let local = tokio::spawn(async move { files::send_file(&mgr, key, &local_path).await });
    let local_stream = Incoming::accept(&client).await;
    client.send(request(&path, ID, 1)).await;
    let remote = Incoming::accept(&client).await;
    client.send(request(&path, ID2, 2)).await;
    assert_status(
        &status(&mut client).await,
        ID2,
        2,
        State::Failed,
        FileError::Busy,
    );
    let error = files::send_file(&host.mgr, key, &path).await.unwrap_err();
    assert!(
        matches!(error, file_send::SendError::Io(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
    client
        .send(pb::FileCancel {
            transfer_id: local_stream.header.transfer_id.clone(),
        })
        .await;
    assert_status(
        &status(&mut client).await,
        &local_stream.header.transfer_id,
        0,
        State::Cancelled,
        FileError::None,
    );
    assert_eq!(
        timeout(Duration::from_secs(3), local)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        file_send::SendOutcome::Cancelled
    );
    assert_eq!(remote.finish().await.len(), 16 * MIB);
    host.wait_until(|| downloads(&host).len() == 1).await;
}
