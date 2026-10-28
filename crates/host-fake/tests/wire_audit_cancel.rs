use host_core::audit::{AuditPath, HostAuditRecord};
use host_core::session::{handlers::roster, journal::JOURNAL_FILE, takeover::Seat};
use rsp_core::pb;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

mod wire_support;
use wire_support::{client_hello, device_id, Dev, OffsetClock, WireHost, WireOptions};

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "rs-b11g-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn start(dir: &Path, clock: &Arc<OffsetClock>) -> WireHost {
    WireHost::start_with(WireOptions {
        audit: true,
        listeners: 2,
        state_dir: Some(dir.to_path_buf()),
        clock: Some(clock.clone()),
        ..Default::default()
    })
    .await
}

async fn stop(host: WireHost) {
    let weak = Arc::downgrade(&host.mgr);
    let started = Instant::now();
    drop(host);
    tokio::time::timeout(Duration::from_secs(5), async {
        while weak.strong_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Host tasks still hold SessionManager after 5 seconds");
    eprintln!(
        "stop(): SessionManager released after {:?}",
        started.elapsed()
    );
}

fn journal(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join(JOURNAL_FILE)).unwrap()).unwrap()
}

fn session_events(host: &WireHost) -> Vec<HostAuditRecord> {
    host.audit_events()
        .into_iter()
        .filter(|record| {
            matches!(
                record.event.as_str(),
                "session_start" | "session_end" | "role_change"
            )
        })
        .collect()
}

fn lingering(host: &WireHost) -> bool {
    matches!(roster::seat(&host.mgr), Seat::Lingering { .. }) && host.conns().is_empty()
}

async fn files_auth(client: &mut wire_support::WireClient) -> pb::AuthResult {
    client
        .send(pb::ClientHello {
            intent: pb::Intent::Files as i32,
            ..client_hello()
        })
        .await;
    loop {
        match client.next().await {
            pb::envelope::Msg::AuthResult(auth) => return auth,
            pb::envelope::Msg::ServerHello(_) => {}
            other => panic!("unexpected FILES handshake message: {other:?}"),
        }
    }
}

#[tokio::test]
async fn cancelled_connection_ends_after_linger_expiry() {
    let dir = Dir::new();
    let clock = Arc::new(OffsetClock::manual());
    let host = start(&dir.0, &clock).await;
    let mut a = host.connect_as(Dev::A, 1).await;
    let (_, auth) = a.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    let since_us = host.conns()[0].since_us;
    clock.skip(Duration::from_secs(5));
    host.abort_listener(1);
    host.wait_until(|| lingering(&host)).await;
    let events = session_events(&host);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, "session_start");
    assert_eq!(
        journal(&dir.0)["sessions"][0]["left_at_us"],
        json!(since_us + 5_000_000)
    );

    clock.skip(Duration::from_secs(61));
    let wait_started = Instant::now();
    host.wait_until(|| session_events(&host).len() == 2).await;
    eprintln!(
        "cancelled_connection_ends_after_linger_expiry: skip(61s) to session_end took {:?}",
        wait_started.elapsed()
    );
    let events = session_events(&host);
    let end = &events[1];
    assert_eq!(end.event, "session_end");
    assert_eq!(end.duration_s, Some(5));
    assert_eq!(end.detail, Some(json!({"role": "controller"})));
    assert_eq!(end.path, Some(AuditPath::Lan));
    assert_eq!(end.device_id, Some(device_id(Dev::A)));
    assert_eq!(journal(&dir.0)["sessions"], json!([]));
    assert!(roster::summaries(&host.mgr).is_empty());
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(session_events(&host).len(), 2);
}

#[tokio::test]
async fn cancelled_connection_resumes_without_extra_end() {
    let host = WireHost::start_with(WireOptions {
        audit: true,
        listeners: 2,
        ..Default::default()
    })
    .await;
    let mut a = host.connect_as(Dev::A, 1).await;
    let (_, auth) = a.hello().await;
    let original = host.conns()[0].clone();
    host.clock.skip(Duration::from_secs(3));
    host.abort_listener(1);
    host.wait_until(|| lingering(&host)).await;
    host.clock.skip(Duration::from_secs(10));

    let mut resumed = host.connect_as(Dev::A, 0).await;
    let (_, resumed_auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token,
            ..client_hello()
        })
        .await;
    assert_eq!(resumed_auth.status, pb::auth_result::Status::Ok as i32);
    assert!(resumed_auth.resumed);
    assert_eq!(host.conns().len(), 1);
    assert_eq!(host.conns()[0].session_id, original.session_id);
    assert_eq!(host.conns()[0].since_us, original.since_us);
    assert_eq!(session_events(&host).len(), 1);

    host.clock.skip(Duration::from_secs(2));
    resumed.goodbye(pb::goodbye::Reason::UserClosed).await;
    host.wait_until(|| session_events(&host).len() == 2).await;
    let events = session_events(&host);
    assert_eq!(events[1].event, "session_end");
    assert_eq!(events[1].duration_s, Some(15));
    assert_eq!(journal(&host.state_dir)["sessions"], json!([]));
}

#[tokio::test]
async fn cancelled_then_host_restart_recovers_once() {
    let dir = Dir::new();
    let clock = Arc::new(OffsetClock::manual());
    let h1 = start(&dir.0, &clock).await;
    let mut a = h1.connect_as(Dev::A, 1).await;
    let (_, auth) = a.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    let since_us = h1.conns()[0].since_us;
    clock.skip(Duration::from_secs(5));
    h1.abort_listener(1);
    h1.wait_until(|| lingering(&h1)).await;
    clock.skip(Duration::from_secs(20));
    stop(h1).await;

    let h2 = start(&dir.0, &clock).await;
    let events = session_events(&h2);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event, "session_start");
    assert_eq!(events[1].event, "session_end");
    assert_eq!(events[1].duration_s, Some(5));
    assert_eq!(
        events[1].detail,
        Some(json!({
            "role": "controller",
            "recovered": true,
            "ended_at": since_us + 5_000_000,
        }))
    );
    assert_eq!(journal(&dir.0)["sessions"], json!([]));
    assert!(roster::summaries(&h2.mgr).is_empty());
    stop(h2).await;

    let h3 = start(&dir.0, &clock).await;
    assert_eq!(session_events(&h3).len(), 2);
    stop(h3).await;
}

#[tokio::test]
async fn viewer_cancel_keeps_role() {
    let host = WireHost::start_with(WireOptions {
        audit: true,
        listeners: 2,
        ..Default::default()
    })
    .await;
    let mut a = host.connect_as(Dev::A, 0).await;
    a.hello().await;
    let mut b = host.connect_as(Dev::B, 1).await;
    let (_, auth_b) = b.hello().await;
    assert_eq!(auth_b.role, pb::Role::Viewer as i32);
    host.clock.skip(Duration::from_secs(4));
    host.abort_listener(1);
    host.wait_until(|| host.conns().len() == 1).await;
    assert_eq!(host.conns()[0].device_id, device_id(Dev::A));
    host.clock.skip(Duration::from_secs(61));
    host.wait_until(|| {
        session_events(&host).iter().any(|event| {
            event.event == "session_end"
                && event.device_id.as_deref() == Some(device_id(Dev::B).as_str())
        })
    })
    .await;
    let end = session_events(&host)
        .into_iter()
        .find(|event| {
            event.event == "session_end"
                && event.device_id.as_deref() == Some(device_id(Dev::B).as_str())
        })
        .unwrap();
    assert_eq!(end.duration_s, Some(4));
    assert_eq!(end.detail, Some(json!({"role": "viewer"})));
    assert_eq!(host.conns().len(), 1);
    assert!(session_events(&host).iter().all(|event| {
        event.device_id.as_deref() != Some(device_id(Dev::A).as_str())
            || event.event != "session_end"
    }));
}

#[tokio::test]
async fn files_connection_cancel_writes_nothing() {
    let host = WireHost::start_with(WireOptions {
        audit: true,
        listeners: 2,
        ..Default::default()
    })
    .await;
    let mut a = host.connect_as(Dev::A, 1).await;
    let auth = files_auth(&mut a).await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    host.abort_listener(1);
    host.wait_until(|| host.conns().is_empty()).await;
    host.clock.skip(Duration::from_secs(61));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(session_events(&host).is_empty());
    assert!(roster::summaries(&host.mgr).is_empty());
}
