//! SFTP browsing against an in-process rooted SFTP server on loopback.

mod support;

use std::{
    fs,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rs_ssh::sftp::{DirEntry, EntryKind, Sftp, SftpError};
use support::sftp_server::SftpTestServer;
use tokio::{io::AsyncWriteExt as _, time::timeout};

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn write_file(path: &Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
}

async fn open(server: &SftpTestServer) -> (rs_ssh::SshSession, Sftp) {
    let session = server.connect().await;
    let sftp = session.sftp().await.unwrap();
    (session, sftp)
}

fn entry<'a>(entries: &'a [DirEntry], name: &str) -> &'a DirEntry {
    entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("missing entry {name}: {entries:?}"))
}

#[tokio::test]
async fn home_and_server_side_canonicalization() {
    let server = SftpTestServer::start_temporary("home").await;
    let (session, sftp) = open(&server).await;

    assert_eq!(sftp.home().await.unwrap(), "/home/test");
    assert_eq!(
        sftp.canonicalize("/home/test/../test/a").await.unwrap(),
        "/home/test/a"
    );

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn list_sorts_directories_first_and_reports_metadata() {
    let server = SftpTestServer::start_temporary("list").await;
    fs::create_dir(server.root().join("B")).unwrap();
    fs::create_dir(server.root().join("a")).unwrap();
    write_file(&server.root().join("c.txt"), b"hello");
    write_file(&server.root().join("D.md"), b"markdown");
    std::os::unix::fs::symlink("c.txt", server.root().join("ln")).unwrap();

    let (session, sftp) = open(&server).await;
    let entries = sftp.list("/home/test").await.unwrap();
    let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["a", "B", "c.txt", "D.md", "ln"]);

    let a = entry(&entries, "a");
    assert_eq!(a.kind, EntryKind::Dir);
    let b = entry(&entries, "B");
    assert_eq!(b.kind, EntryKind::Dir);

    for name in ["c.txt", "D.md", "ln"] {
        let listed = entry(&entries, name);
        assert_eq!(listed.kind == EntryKind::Symlink, name == "ln");
        if name != "ln" {
            let local = fs::metadata(server.root().join(name)).unwrap();
            assert_eq!(listed.size, Some(local.len()));
            assert_eq!(listed.mtime, Some(seconds(local.modified().unwrap())));
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(listed.permissions, Some(local.permissions().mode()));
        }
    }
    assert_eq!(entry(&entries, "c.txt").size, Some(5));
    assert_eq!(entry(&entries, "c.txt").kind, EntryKind::File);
    assert!(!names.contains(&"."));
    assert!(!names.contains(&".."));

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn list_reads_all_server_pages() {
    let server = SftpTestServer::start_temporary("pages").await;
    for index in 0..250 {
        write_file(&server.root().join(format!("file-{index:03}")), b"x");
    }

    let (session, sftp) = open(&server).await;
    let entries = sftp.list("/home/test").await.unwrap();
    assert_eq!(entries.len(), 250);
    assert!(entries.windows(2).all(|pair| pair[0].name < pair[1].name));

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn stat_errors_and_root_boundary() {
    let server = SftpTestServer::start_temporary("stat").await;
    fs::create_dir(server.root().join("dir")).unwrap();
    write_file(&server.root().join("file"), b"abc");

    let (session, sftp) = open(&server).await;
    let file = sftp.stat("/home/test/file").await.unwrap();
    assert_eq!(file.name, "file");
    assert_eq!(file.kind, EntryKind::File);
    assert_eq!(file.size, Some(3));

    let dir = sftp.stat("/home/test/dir").await.unwrap();
    assert_eq!(dir.name, "dir");
    assert_eq!(dir.kind, EntryKind::Dir);

    assert!(matches!(
        sftp.stat("/home/test/missing").await,
        Err(SftpError::NotFound)
    ));
    assert!(matches!(
        sftp.list("/root").await,
        Err(SftpError::PermissionDenied)
    ));
    assert!(matches!(
        sftp.list("/home/test/../../..").await,
        Err(SftpError::PermissionDenied)
    ));

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn mkdir_rename_and_remove_are_reflected_locally() {
    let server = SftpTestServer::start_temporary("mutate").await;
    write_file(&server.root().join("source.txt"), b"source");
    fs::create_dir(server.root().join("nonempty")).unwrap();
    write_file(&server.root().join("nonempty/child"), b"child");
    fs::create_dir(server.root().join("empty")).unwrap();

    let (session, sftp) = open(&server).await;
    sftp.mkdir("/home/test/new").await.unwrap();
    assert!(server.root().join("new").is_dir());
    assert!(matches!(
        sftp.mkdir("/home/test/new").await,
        Err(SftpError::AlreadyExists)
    ));

    sftp.rename("/home/test/source.txt", "/home/test/renamed.txt")
        .await
        .unwrap();
    assert!(!server.root().join("source.txt").exists());
    assert_eq!(
        fs::read(server.root().join("renamed.txt")).unwrap(),
        b"source"
    );

    sftp.remove_file("/home/test/renamed.txt").await.unwrap();
    assert!(!server.root().join("renamed.txt").exists());

    assert!(matches!(
        sftp.remove_dir("/home/test/nonempty").await,
        Err(SftpError::NotEmpty)
    ));
    assert!(server.root().join("nonempty/child").exists());
    sftp.remove_dir("/home/test/empty").await.unwrap();
    assert!(!server.root().join("empty").exists());

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn subsystem_rejection_is_unavailable() {
    let root = support::sftp_server::unique_temp_dir("without-sftp");
    let server = SftpTestServer::without_sftp(root).await;
    let session = server.connect().await;

    assert!(matches!(session.sftp().await, Err(SftpError::Unavailable)));

    session.close().await;
}

#[tokio::test]
async fn operations_fail_promptly_after_session_close() {
    let server = SftpTestServer::start_temporary("closed").await;
    let (session, sftp) = open(&server).await;
    session.close().await;

    let result = timeout(Duration::from_secs(10), sftp.list("/home/test"))
        .await
        .expect("list after close must not hang");
    assert!(matches!(result, Err(SftpError::Ssh(_))), "{result:?}");
    sftp.close().await;
}

#[tokio::test]
async fn fixture_io_round_trip_and_fault_injection() {
    let server = SftpTestServer::start_temporary("io").await;
    let (session, sftp) = open(&server).await;
    let direct = support::sftp_server::RawSftp::connect(&server).await;
    let mut payload = vec![0u8; 1024 * 1024];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    {
        let mut file = direct.create("/home/test/blob").await.unwrap();
        file.write_all(&payload).await.unwrap();
        file.close().await.unwrap();
    }
    {
        use tokio::io::AsyncReadExt as _;
        let mut file = direct.open_file("/home/test/blob").await.unwrap();
        let mut read_back = Vec::new();
        file.read_to_end(&mut read_back).await.unwrap();
        assert_eq!(read_back, payload);
    }

    let read_requests = server.read_requests();
    assert!(!read_requests.is_empty());
    assert_eq!(read_requests[0].path, "/home/test/blob");
    assert_eq!(read_requests[0].offset, 0);
    let read_total: u64 = read_requests.iter().map(|record| record.len).sum();
    assert!(read_total >= payload.len() as u64);

    server.fail_after_read_bytes(Some(64 * 1024));
    use tokio::io::AsyncReadExt as _;
    let mut file = direct.open_file("/home/test/blob").await.unwrap();
    let mut read_back = Vec::new();
    let result = timeout(Duration::from_secs(10), file.read_to_end(&mut read_back)).await;
    let failed = match result {
        Ok(Ok(_)) => false,
        Ok(Err(_)) | Err(_) => true,
    };
    assert!(failed, "injected read fault did not break the stream");
    assert!(
        timeout(Duration::from_secs(10), direct.list_len("/home/test"))
            .await
            .expect("faulted connection must not hang")
            .is_err()
    );

    let requests = server.read_requests();
    assert_eq!(requests[requests.len() - 1].path, "/home/test/blob");
    assert!(requests
        .windows(2)
        .any(|pair| pair[1].offset >= pair[0].offset + pair[0].len));

    sftp.close().await;
    direct.close().await;
    session.close().await;
}

#[tokio::test]
async fn fixture_setstat_and_fsetstat_apply_attributes() {
    use std::os::unix::fs::PermissionsExt as _;

    let server = SftpTestServer::start_temporary("attrs").await;
    write_file(&server.root().join("attrs.bin"), b"abcdef");
    let direct = support::sftp_server::RawSftp::connect(&server).await;

    let attrs = russh_sftp::protocol::FileAttributes {
        size: Some(3),
        permissions: Some(0o640),
        mtime: Some(1_700_000_000),
        ..Default::default()
    };
    direct
        .set_attrs("/home/test/attrs.bin", attrs)
        .await
        .unwrap();

    let metadata = fs::metadata(server.root().join("attrs.bin")).unwrap();
    assert_eq!(metadata.len(), 3);
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o640);
    assert_eq!(seconds(metadata.modified().unwrap()), 1_700_000_000);

    let file = direct.open_file("/home/test/attrs.bin").await.unwrap();
    let attrs = russh_sftp::protocol::FileAttributes {
        size: Some(1),
        ..Default::default()
    };
    file.set_metadata(attrs).await.unwrap();
    assert_eq!(
        fs::metadata(server.root().join("attrs.bin")).unwrap().len(),
        1
    );

    direct.close().await;
}

#[tokio::test]
async fn fixture_faults_are_scoped_to_individual_connections() {
    let server = SftpTestServer::start_temporary("connections").await;
    write_file(&server.root().join("blob"), b"abcdefgh");
    let first = support::sftp_server::RawSftp::connect(&server).await;
    let second = support::sftp_server::RawSftp::connect(&server).await;
    let faults = server.connection_faults();
    assert_eq!(faults.len(), 2);

    faults[0].fail_after_read_bytes(Some(2));
    {
        use tokio::io::AsyncReadExt as _;
        let mut file = first.open_file("/home/test/blob").await.unwrap();
        let mut data = Vec::new();
        assert!(
            timeout(Duration::from_secs(10), file.read_to_end(&mut data))
                .await
                .expect("faulted connection must not hang")
                .is_err()
        );
    }
    assert_eq!(second.list_len("/home/test").await.unwrap(), 1);
    assert!(faults[0]
        .read_requests()
        .iter()
        .any(|record| record.path == "/home/test/blob"));
    assert!(faults[1].read_requests().is_empty());

    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn stat_name_uses_last_path_component() {
    let server = SftpTestServer::start_temporary("name").await;
    write_file(&server.root().join("name.txt"), b"x");
    let (session, sftp) = open(&server).await;

    assert_eq!(
        sftp.stat("/home/test/name.txt").await.unwrap().name,
        "name.txt"
    );
    assert_eq!(sftp.stat("/home/test").await.unwrap().name, "test");
    assert_eq!(
        sftp.lstat("/home/test/name.txt").await.unwrap().name,
        "name.txt"
    );
    assert_eq!(sftp.stat("/home/test/").await.unwrap().name, "test");

    sftp.close().await;
    session.close().await;
}
