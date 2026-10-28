//! Owner login resolution from the local Tailscale status.

mod wire_support;

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use host_core::{
    owner::{self, OwnerOptions, OwnerSource},
    whois::{parse_self_login, WhoIsError},
};
use rsp_core::pb;
use serde_json::json;
use tokio::time::{sleep, timeout};
use wire_support::*;

const LOGIN: &str = "alice@example.com";

struct FakeOwner {
    results: Mutex<VecDeque<Result<String, WhoIsError>>>,
    calls: AtomicUsize,
}
impl FakeOwner {
    fn new(results: Vec<Result<String, WhoIsError>>) -> Arc<Self> {
        assert!(!results.is_empty());
        Arc::new(Self {
            results: Mutex::new(results.into()),
            calls: AtomicUsize::new(0),
        })
    }
}
#[async_trait]
impl OwnerSource for FakeOwner {
    async fn self_login(&self) -> Result<String, WhoIsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut results = self.results.lock().unwrap();
        if results.len() > 1 {
            results.pop_front().unwrap()
        } else {
            results.front().unwrap().clone()
        }
    }
}
fn unavailable() -> Result<String, WhoIsError> {
    Err(WhoIsError::Timeout)
}
fn owner_options(owner: Arc<FakeOwner>) -> OwnerOptions {
    OwnerOptions {
        owner,
        owner_retry: Duration::from_millis(100),
    }
}

async fn wait_for(mut ready: impl FnMut() -> bool, within: Duration) {
    timeout(within, async {
        while !ready() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition met before deadline");
}

#[test]
fn self_login_uses_only_the_self_user() {
    assert_eq!(
        parse_self_login(&json!({"Self":{"UserID":7},"User":{"7":{"LoginName":LOGIN}}})).unwrap(),
        LOGIN
    );
    for status in [
        json!({}),
        json!({"Self":{"UserID":7},"User":{"8":{"LoginName":LOGIN}}}),
        json!({"Self":{"UserID":0},"User":{"0":{"LoginName":LOGIN}}}),
        json!({"Self":{"UserID":7},"User":{"7":{"LoginName":""}}}),
        json!({"Self":{"UserID":7},"User":{"7":{"LoginName":"  "}}}),
        json!({"Self":{"UserID":"7"},"User":{"7":{"LoginName":LOGIN}}}),
        json!({"Self":{"Tags":["tag:rs-host"]},"User":{"7":{"LoginName":LOGIN}}}),
    ] {
        assert!(matches!(
            parse_self_login(&status),
            Err(WhoIsError::Json(_))
        ));
    }
}

#[tokio::test]
async fn retries_owner_until_it_resolves() {
    let host = WireHost::start().await;
    let owner = FakeOwner::new(vec![unavailable(), unavailable(), Ok(LOGIN.into())]);
    let task = owner::spawn(host.mgr.clone(), owner_options(owner.clone()));
    wait_for(
        || owner.calls.load(Ordering::SeqCst) == 1,
        Duration::from_secs(1),
    )
    .await;
    assert!(host.mgr.owner_login().is_none());
    wait_for(
        || owner.calls.load(Ordering::SeqCst) == 2,
        Duration::from_secs(1),
    )
    .await;
    assert!(host.mgr.owner_login().is_none());
    timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(host.mgr.owner_login().as_deref(), Some(LOGIN));
    assert_eq!(owner.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn empty_owner_is_retried_and_empty_allowed_users_uses_resolved_owner() {
    let host = WireHost::start_with(WireOptions {
        tailnet_users: Some(vec![]),
        ..Default::default()
    })
    .await;
    let owner = FakeOwner::new(vec![Ok(String::new()), Ok(LOGIN.into())]);
    let task = owner::spawn(host.mgr.clone(), owner_options(owner.clone()));
    wait_for(
        || owner.calls.load(Ordering::SeqCst) == 1,
        Duration::from_secs(1),
    )
    .await;
    assert!(host.mgr.owner_login().is_none());
    timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    let mut alice = host.connect_user(Dev::A, LOGIN).await;
    assert_eq!(
        alice.hello().await.1.status,
        pb::auth_result::Status::Ok as i32
    );
    let mut bob = host.connect_user(Dev::B, "bob@example.com").await;
    let result = bob.hello().await.1;
    assert_eq!(result.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(result.error.unwrap().code, 130);
}

#[tokio::test]
async fn dropping_manager_stops_owner_retries() {
    let host = WireHost::start().await;
    let owner = FakeOwner::new(vec![unavailable()]);
    let task = owner::spawn(host.mgr.clone(), owner_options(owner.clone()));
    wait_for(
        || owner.calls.load(Ordering::SeqCst) > 0,
        Duration::from_secs(1),
    )
    .await;
    let weak = Arc::downgrade(&host.mgr);
    drop(host);
    timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(weak.upgrade().is_none());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn local_status_and_cli_fallback_are_queried_fresh() {
    use host_core::whois::{ChainedWhoIs, CliSource, LocalApiSource};
    use std::os::{fd::AsRawFd, unix::fs::PermissionsExt};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    let host = WireHost::start().await;
    // A descriptor path avoids the Unix socket path limit with the shared TMPDIR.
    let directory = std::fs::File::open(&host.state_dir).unwrap();
    let socket = format!("/proc/self/fd/{}/s", directory.as_raw_fd());
    let listener = UnixListener::bind(&socket).unwrap();
    let executable = host.state_dir.join("tailscale");
    std::fs::write(&executable, "#!/bin/sh\n[ \"$1\" = status ] && [ \"$2\" = --json ] || exit 1\nprintf '%s' '{\"Self\":{\"UserID\":8},\"User\":{\"8\":{\"LoginName\":\"bob@example.com\"}}}'\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = ChainedWhoIs::new(LocalApiSource::new(socket), CliSource::new(executable));
    let server = tokio::spawn(async move {
        for login in [Some(LOGIN), None, Some("carol@example.com")] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            let size = stream.read(&mut request).await.unwrap();
            assert!(std::str::from_utf8(&request[..size])
                .unwrap()
                .starts_with("GET /localapi/v0/status HTTP/1.1\r\n"));
            let body = match login {
                Some(login) => json!({"Self":{"UserID":7},"User":{"7":{"LoginName":login}}}),
                None => json!({"Self":{"UserID":0}}),
            }
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    assert_eq!(source.self_login().await.unwrap(), LOGIN);
    assert_eq!(source.self_login().await.unwrap(), "bob@example.com");
    assert_eq!(source.self_login().await.unwrap(), "carol@example.com");
    server.await.unwrap();
}
