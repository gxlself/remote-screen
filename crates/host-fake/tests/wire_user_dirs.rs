use host_core::file_send;
use host_core::user_dirs::{DirsOwner, UserDirs, UserDirsSource};
use rsp_core::{
    file::{build_header, encode_stream_prefix},
    file_ctl,
    pb::{self, file_status::State, FileError, FileHeader, FileListing, FileStatus},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::time::timeout;

mod wire_support;
use wire_support::{WireClient, WireHost};

const ID: &str = "11111111111111111111111111111111";
const ID2: &str = "22222222222222222222222222222222";
const ID3: &str = "33333333333333333333333333333333";

struct FakeDirs {
    dirs: Mutex<UserDirs>,
    can_write: AtomicBool,
}

impl FakeDirs {
    fn new(dirs: UserDirs) -> Arc<Self> {
        Arc::new(Self {
            dirs: Mutex::new(dirs),
            can_write: AtomicBool::new(true),
        })
    }

    fn set_dirs(&self, dirs: UserDirs) {
        *self.dirs.lock().unwrap() = dirs;
    }
}

impl UserDirsSource for FakeDirs {
    fn current(&self) -> UserDirs {
        self.dirs.lock().unwrap().clone()
    }

    fn user_can_write(&self, _dir: &Path) -> bool {
        self.can_write.load(Ordering::Acquire)
    }
}

fn dirs(downloads: PathBuf, home: Option<PathBuf>) -> UserDirs {
    UserDirs {
        downloads,
        home,
        owner: DirsOwner::User,
    }
}

fn external_home(host: &WireHost, name: &str) -> PathBuf {
    host.state_dir
        .parent()
        .unwrap()
        .join(format!("rs-user-dirs-{name}-{}", std::process::id()))
}

async fn header(host: &WireHost, id: &str, body: &[u8]) -> FileHeader {
    let path = host.state_dir.join(format!("hash-{id}"));
    tokio::fs::write(&path, body).await.unwrap();
    let prepared = file_send::prepare(&path, Some(id.to_owned()))
        .await
        .unwrap();
    tokio::fs::remove_file(path).await.unwrap();
    build_header(
        id,
        "received.txt",
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
        .next_skipping_session_state("FileStatus", |msg| match msg {
            pb::envelope::Msg::FileStatus(status) => Some(status.clone()),
            _ => None,
        })
        .await
}

async fn listing(client: &mut WireClient, path: &str, request_id: u32) -> FileListing {
    client
        .send(pb::FileList {
            request_id,
            path: path.to_owned(),
            offset: 0,
        })
        .await;
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::FileListing(listing) if listing.request_id == request_id => {
                Some(listing.clone())
            }
            _ => None,
        })
        .await
}

fn install(host: &WireHost, fake: &Arc<FakeDirs>) {
    assert!(host.mgr.features().files.set_user_dirs(fake.clone()));
}

#[tokio::test]
async fn received_files_follow_the_current_download_directory() {
    let host = WireHost::start().await;
    let a = host.state_dir.join("downloads-a");
    let b = host.state_dir.join("downloads-b");
    let fake = FakeDirs::new(dirs(a.clone(), None));
    install(&host, &fake);
    let mut client = host.connect().await;
    client.hello().await;

    let first = b"first";
    upload(&client, &header(&host, ID, first).await, first).await;
    assert_eq!(status(&mut client).await.state, State::Completed as i32);
    assert_eq!(
        tokio::fs::read(a.join("received.txt")).await.unwrap(),
        first
    );

    std::fs::create_dir_all(&b).unwrap();
    fake.set_dirs(dirs(b.clone(), None));
    let second = b"second";
    upload(&client, &header(&host, ID2, second).await, second).await;
    assert_eq!(status(&mut client).await.state, State::Completed as i32);
    assert_eq!(
        tokio::fs::read(b.join("received.txt")).await.unwrap(),
        second
    );
    assert_eq!(
        tokio::fs::read(a.join("received.txt")).await.unwrap(),
        first
    );
}

#[tokio::test]
async fn file_list_uses_the_resolved_home() {
    let host = WireHost::start().await;
    let home = external_home(&host, "list");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("docs")).unwrap();
    let fake = FakeDirs::new(dirs(host.state_dir.join("downloads"), Some(home.clone())));
    install(&host, &fake);
    let mut client = host.connect().await;
    client.hello().await;

    let result = listing(&mut client, "", 1).await;
    assert_eq!(result.error, FileError::None as i32);
    assert_eq!(result.path, home.canonicalize().unwrap().to_string_lossy());
    assert!(result.entries.iter().any(|entry| entry.name == "docs"));
}

#[tokio::test]
async fn file_list_is_denied_without_a_home() {
    let host = WireHost::start().await;
    let fake = FakeDirs::new(dirs(host.state_dir.join("downloads"), None));
    install(&host, &fake);
    let mut client = host.connect().await;
    client.hello().await;

    assert_eq!(
        listing(&mut client, "", 1).await.error,
        FileError::PermissionDenied as i32
    );
}

#[tokio::test]
async fn destination_write_permission_is_checked() {
    let host = WireHost::start().await;
    let home = external_home(&host, "write");
    let _ = std::fs::remove_dir_all(&home);
    let docs = home.join("docs");
    std::fs::create_dir_all(&docs).unwrap();
    let fake = FakeDirs::new(dirs(host.state_dir.join("downloads"), Some(home)));
    fake.can_write.store(false, Ordering::Release);
    install(&host, &fake);
    let mut client = host.connect().await;
    client.hello().await;

    let body = b"denied";
    let mut denied = header(&host, ID, body).await;
    denied.dest_dir = docs.to_string_lossy().into_owned();
    let mut stream = client.open_typed(0x04).await;
    stream
        .write_all(&encode_stream_prefix(&denied).unwrap())
        .await
        .unwrap();
    let result = status(&mut client).await;
    assert_eq!(result.state, State::Failed as i32);
    assert_eq!(result.error, FileError::PermissionDenied as i32);
    assert_eq!(
        timeout(Duration::from_secs(3), stream.stopped())
            .await
            .unwrap()
            .unwrap(),
        Some(file_ctl::STREAM_REJECTED.into())
    );
    assert!(std::fs::read_dir(&docs).unwrap().next().is_none());

    fake.can_write.store(true, Ordering::Release);
    let mut allowed = header(&host, ID2, body).await;
    allowed.dest_dir = docs.to_string_lossy().into_owned();
    upload(&client, &allowed, body).await;
    let result = status(&mut client).await;
    assert_eq!(result.state, State::Completed as i32);
    assert_eq!(
        tokio::fs::read(docs.join("received.txt")).await.unwrap(),
        body
    );
}

#[tokio::test]
async fn user_directory_resolver_can_only_be_set_once() {
    let host = WireHost::start().await;
    let first = FakeDirs::new(dirs(host.state_dir.join("first"), None));
    let second = FakeDirs::new(dirs(host.state_dir.join("second"), None));
    assert!(host.mgr.features().files.set_user_dirs(first.clone()));
    assert!(!host.mgr.features().files.set_user_dirs(second));
    first.set_dirs(dirs(host.state_dir.join("updated"), None));

    let mut client = host.connect().await;
    client.hello().await;
    let body = b"once";
    upload(&client, &header(&host, ID3, body).await, body).await;
    assert_eq!(status(&mut client).await.state, State::Completed as i32);
    assert!(host.state_dir.join("updated/received.txt").exists());
    assert!(!host.state_dir.join("second/received.txt").exists());
}
