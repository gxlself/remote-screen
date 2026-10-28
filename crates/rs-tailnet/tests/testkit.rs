#![cfg(all(unix, feature = "testkit"))]

use rs_tailnet::testkit::{temp_dir, TestNet, TestNetOptions, TestPaths};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

struct Script {
    dir: PathBuf,
    paths: TestPaths,
}
impl Script {
    fn new(body: &str) -> Self {
        let dir = temp_dir("helper-script");
        let path = dir.join("helper");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            paths: TestPaths {
                lib: PathBuf::new(),
                testnet: path,
            },
            dir,
        }
    }
}
impl Drop for Script {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn announcement(admin: &str) -> String {
    format!(
        r#"echo 'RS_TESTNET {{"controlUrl":"http://127.0.0.1:1","adminUrl":"{admin}","authKey":"tskey-fixture","peer":{{"hostname":"rs-peer","dnsName":"rs-peer.rs.test.","ips":["100.64.0.1"]}},"udpEcho":7401,"tcpEcho":7402}}'"#
    )
}

#[test]
fn helper_drains_pipes_and_drop_closes_stdin() {
    let script = Script::new(&format!("{}\ni=0\nwhile [ $i -lt 6000 ]; do\n echo 'stdout diagnostic stdout diagnostic stdout diagnostic'\n echo 'stderr diagnostic stderr diagnostic stderr diagnostic' >&2\n i=$((i+1))\ndone\nread line\necho done > \"$0.done\"", announcement("http://127.0.0.1:1")));
    let helper = TestNet::spawn(&script.paths, &TestNetOptions::default()).unwrap();
    assert_eq!(helper.peer_ip4().to_string(), "100.64.0.1");
    assert_eq!(helper.peer_hostname, "rs-peer");
    let start = Instant::now();
    drop(helper);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(script.paths.testnet.with_extension("done").exists());
}

#[test]
fn helper_startup_errors_include_redacted_tails() {
    let script = Script::new(
        "echo 'out custom-secret'\necho 'err tskey-generated-token custom-secret' >&2\nexit 1",
    );
    let options = TestNetOptions {
        auth_key: Some("custom-secret".into()),
        ..Default::default()
    };
    let error = match TestNet::spawn(&script.paths, &options) {
        Ok(_) => panic!("helper unexpectedly started"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(message.contains("stdout tail: out <redacted>"));
    assert!(message.contains("stderr tail: err <redacted> <redacted>"));
    assert!(!message.contains("custom-secret"));
    assert!(!message.contains("tskey-generated-token"));
}

#[test]
fn malformed_announcement_does_not_expose_key() {
    let script = Script::new("echo 'RS_TESTNET {\"authKey\":\"tskey-secret\"}'\nread line");
    let error = match TestNet::spawn(&script.paths, &TestNetOptions::default()) {
        Ok(_) => panic!("invalid announcement accepted"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(!error.to_string().contains("tskey-secret"));
}

#[test]
fn complete_posts_auth_url_and_rejects_non_200() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let admin = format!("http://{}", listener.local_addr().unwrap());
    let script = Script::new(&format!("{}\nread line", announcement(&admin)));
    let helper = TestNet::spawn(&script.paths, &TestNetOptions::default()).unwrap();
    let server = thread::spawn(move || {
        for code in [200, 403] {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            assert_eq!(first, "POST /complete HTTP/1.1\r\n");
            let mut length = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length: ") {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; length.unwrap()];
            reader.read_exact(&mut body).unwrap();
            assert_eq!(body, b"http://hs.example.com/auth/fixture");
            write!(
                stream,
                "HTTP/1.1 {code} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        }
    });
    helper
        .complete("http://hs.example.com/auth/fixture")
        .unwrap();
    assert!(helper
        .complete("http://hs.example.com/auth/fixture")
        .is_err());
    server.join().unwrap();
}

#[test]
fn temp_directories_are_unique_and_created() {
    let first = temp_dir("same/tag");
    let second = temp_dir("same/tag");
    assert_ne!(first, second);
    assert!(first.is_dir() && second.is_dir());
    if let Some(root) = std::env::var_os("TMPDIR") {
        assert!(first.starts_with(root));
    }
    fs::remove_dir_all(first).unwrap();
    fs::remove_dir_all(second).unwrap();
}
